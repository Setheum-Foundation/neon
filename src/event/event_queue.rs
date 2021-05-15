use neon_runtime::raw::Env;
use neon_runtime::tsfn::ThreadsafeFunction;

use crate::context::{Context, TaskContext};
use crate::result::NeonResult;

type Callback = Box<dyn FnOnce(Env) + Send + 'static>;

/// Channel for scheduling Rust closures to execute on the JavaScript main thread.
///
/// # Example
///
/// The following example spawns a standard Rust thread to complete a computation
/// and calls back to a JavaScript function asynchronously with the result.
///
/// ```
/// # use neon::prelude::*;
/// # fn fibonacci(_: f64) -> f64 { todo!() }
/// fn async_fibonacci(mut cx: FunctionContext) -> JsResult<JsUndefined> {
///     // These types (`f64`, `Root<JsFunction>`, `Channel`) may all be sent
///     // across threads.
///     let n = cx.argument::<JsNumber>(0)?.value(&mut cx);
///     let callback = cx.argument::<JsFunction>(1)?.root(&mut cx);
///     let channel = cx.channel();
///
///     // Spawn a thread to complete the execution. This will _not_ block the
///     // JavaScript event loop.
///     std::thread::spawn(move || {
///         let result = fibonacci(n);
///
///         // Send a closure as a task to be executed by the JavaScript event
///         // loop. This _will_ block the event loop while executing.
///         channel.send(move |mut cx| {
///             let callback = callback.into_inner(&mut cx);
///             let this = cx.undefined();
///             let null = cx.null();
///             let args = vec![
///                 cx.null().upcast::<JsValue>(),
///                 cx.number(result).upcast(),
///             ];
///
///             callback.call(&mut cx, this, args)?;
///
///             Ok(())
///         });
///     });
///
///     Ok(cx.undefined())
/// }
/// ```

pub struct Channel {
    tsfn: ThreadsafeFunction<Callback>,
    has_ref: bool,
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Channel")
    }
}

impl Channel {
    /// Creates an unbounded channel for scheduling closures on the JavaScript
    /// main thread
    pub fn new<'a, C: Context<'a>>(cx: &mut C) -> Self {
        let tsfn = unsafe { ThreadsafeFunction::new(cx.env().to_raw(), Self::callback) };

        Self {
            tsfn,
            has_ref: true,
        }
    }

    /// Allow the Node event loop to exit while this `Channel` exists.
    /// _Idempotent_
    pub fn unref<'a, C: Context<'a>>(&mut self, cx: &mut C) -> &mut Self {
        self.has_ref = false;

        unsafe { self.tsfn.unref(cx.env().to_raw()) }

        self
    }

    /// Prevent the Node event loop from exiting while this `Channel` exists. (Default)
    /// _Idempotent_
    pub fn reference<'a, C: Context<'a>>(&mut self, cx: &mut C) -> &mut Self {
        self.has_ref = true;

        unsafe { self.tsfn.reference(cx.env().to_raw()) }

        self
    }

    /// Schedules a closure to execute on the JavaScript thread that created this Channel
    /// Panics if there is a libuv error
    pub fn send<F>(&self, f: F)
    where
        F: FnOnce(TaskContext) -> NeonResult<()> + Send + 'static,
    {
        self.try_send(f).unwrap()
    }

    /// Schedules a closure to execute on the JavaScript thread that created this Channel
    /// Returns an `Error` if the task could not be scheduled.
    pub fn try_send<F>(&self, f: F) -> Result<(), SendError>
    where
        F: FnOnce(TaskContext) -> NeonResult<()> + Send + 'static,
    {
        let callback = Box::new(move |env| {
            let env = unsafe { std::mem::transmute(env) };

            // Note: It is sufficient to use `TaskContext`'s `InheritedHandleScope` because
            // N-API creates a `HandleScope` before calling the callback.
            TaskContext::with_context(env, move |cx| {
                let _ = f(cx);
            });
        });

        self.tsfn.call(callback, None).map_err(|_| SendError)
    }

    /// Returns a boolean indicating if this `Channel` will prevent the Node event
    /// loop from exiting.
    pub fn has_ref(&self) -> bool {
        self.has_ref
    }

    // Monomorphized trampoline funciton for calling the user provided closure
    fn callback(env: Option<Env>, callback: Callback) {
        if let Some(env) = env {
            callback(env);
        } else {
            crate::context::internal::IS_RUNNING.with(|v| {
                *v.borrow_mut() = false;
            });
        }
    }
}

/// Error indicating that a closure was unable to be scheduled to execute on the event loop.
pub struct SendError;

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SendError")
    }
}

impl std::fmt::Debug for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for SendError {}
