#![allow(non_upper_case_globals)]

use std::convert::identity;

use napi_ohos::{
    Error, Result, Status,
    bindgen_prelude::{FromNapiValue, JsValuesTupleIntoVec, check_status},
    sys,
    threadsafe_function::{
        ThreadsafeFunction, ThreadsafeFunctionCallJsBackData, ThreadsafeFunctionCallMode,
        ThreadsafeFunctionCallVariant,
    },
};

pub trait ThreadsafeFunctionCalleeHandleExt {
    type T: 'static;
    type Return: 'static + FromNapiValue;
    type CallJsBackArgs: 'static + JsValuesTupleIntoVec;
    type ErrorStatus: AsRef<str> + From<Status>;
    const CALLEE_HANDLED: bool;
    const WEAK: bool;
    const MAX_QUEUE_SIZE: usize;

    fn call_local(&self, value: Self::T)
    -> impl std::future::Future<Output = Result<Self::Return>>;
}

pub trait ThreadsafeFunctionCalleeUnHandleExt {
    type T: 'static;
    type Return: 'static + FromNapiValue;
    type CallJsBackArgs: 'static + JsValuesTupleIntoVec;
    type ErrorStatus: AsRef<str> + From<Status>;
    const CALLEE_HANDLED: bool;
    const WEAK: bool;
    const MAX_QUEUE_SIZE: usize;

    fn call_local(
        &self,
        value: Result<Self::T, Self::ErrorStatus>,
    ) -> impl std::future::Future<Output = Result<Self::Return>>;
}

impl<
    T: 'static,
    Return: 'static + FromNapiValue,
    CallJsBackArgs: 'static + JsValuesTupleIntoVec,
    ErrorStatus: AsRef<str> + From<Status>,
    const Weak: bool,
    const MaxQueueSize: usize,
> ThreadsafeFunctionCalleeHandleExt
    for ThreadsafeFunction<
        T,
        Return,
        CallJsBackArgs,
        ErrorStatus,
        false,
        { Weak },
        { MaxQueueSize },
    >
{
    type T = T;
    type Return = Return;
    type CallJsBackArgs = CallJsBackArgs;
    type ErrorStatus = ErrorStatus;

    const CALLEE_HANDLED: bool = false;
    const WEAK: bool = Weak;
    const MAX_QUEUE_SIZE: usize = MaxQueueSize;

    async fn call_local(&self, value: Self::T) -> Result<Self::Return> {
        let (sender, receiver) = ohos_ffrt::oneshot::channel::<Return>();
        self.handle.with_read_aborted(|aborted| {
            if aborted {
                return Err(Error::from_status(Status::Closing));
            }

            check_status!(
                unsafe {
                    sys::napi_call_threadsafe_function(
                        self.handle.get_raw(),
                        Box::into_raw(Box::new(ThreadsafeFunctionCallJsBackData {
                            data: value,
                            call_variant: ThreadsafeFunctionCallVariant::WithCallback,
                            callback: Box::new(move |d, _| {
                                d.and_then(|d| {
                                    sender
                                        .send(d)
                                        // The only reason for send to return Err is if the receiver isn't listening
                                        // Not hiding the error would result in a napi_fatal_error call, it's safe to ignore it instead.
                                        .or(Ok(()))
                                })
                            }),
                        }))
                        .cast(),
                        ThreadsafeFunctionCallMode::NonBlocking.into(),
                    )
                },
                "Threadsafe function call_async failed"
            )
        })?;
        receiver.await.map_err(|_| {
            Error::new(
                Status::GenericFailure,
                "Receive value from threadsafe function sender failed",
            )
        })
    }
}

impl<
    T: 'static,
    Return: 'static + FromNapiValue,
    CallJsBackArgs: 'static + JsValuesTupleIntoVec,
    ErrorStatus: AsRef<str> + From<Status>,
    const Weak: bool,
    const MaxQueueSize: usize,
> ThreadsafeFunctionCalleeUnHandleExt
    for ThreadsafeFunction<T, Return, CallJsBackArgs, ErrorStatus, true, { Weak }, { MaxQueueSize }>
{
    type T = T;
    type Return = Return;
    type CallJsBackArgs = CallJsBackArgs;
    type ErrorStatus = ErrorStatus;

    const CALLEE_HANDLED: bool = true;
    const WEAK: bool = Weak;
    const MAX_QUEUE_SIZE: usize = MaxQueueSize;

    async fn call_local(&self, value: Result<Self::T, Self::ErrorStatus>) -> Result<Self::Return> {
        let (sender, receiver) = ohos_ffrt::oneshot::channel::<Result<Return>>();
        self.handle.with_read_aborted(|aborted| {
            if aborted {
                return Err(Error::from_status(Status::Closing));
            }

            check_status!(
                unsafe {
                    sys::napi_call_threadsafe_function(
                        self.handle.get_raw(),
                        Box::into_raw(Box::new(ThreadsafeFunctionCallJsBackData {
                            data: value,
                            call_variant: ThreadsafeFunctionCallVariant::WithCallback,
                            callback: Box::new(move |d, _| {
                                d.and_then(|d| {
                                    sender
                                        .send(d)
                                        // The only reason for send to return Err is if the receiver isn't listening
                                        // Not hiding the error would result in a napi_fatal_error call, it's safe to ignore it instead.
                                        .or(Ok(()))
                                })
                            }),
                        }))
                        .cast(),
                        ThreadsafeFunctionCallMode::NonBlocking.into(),
                    )
                },
                "Threadsafe function call_async failed"
            )
        })?;
        receiver
            .await
            .map_err(|_| {
                Error::new(
                    Status::GenericFailure,
                    "Receive value from threadsafe function sender failed",
                )
            })
            .and_then(identity)
    }
}
