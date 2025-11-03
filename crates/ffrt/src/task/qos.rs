#![allow(warnings)]

use ohos_ffrt_sys::{
    ffrt_qos_default_t, ffrt_qos_default_t_ffrt_qos_background,
    ffrt_qos_default_t_ffrt_qos_default, ffrt_qos_default_t_ffrt_qos_inherit,
    ffrt_qos_default_t_ffrt_qos_user_initiated, ffrt_qos_default_t_ffrt_qos_utility,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qos {
    Inherit,
    Background,
    Utility,
    Default,
    UserInitiated,
}

impl From<Qos> for ffrt_qos_default_t {
    fn from(qos: Qos) -> Self {
        match qos {
            Qos::Inherit => ffrt_qos_default_t_ffrt_qos_inherit,
            Qos::Background => ffrt_qos_default_t_ffrt_qos_background,
            Qos::Utility => ffrt_qos_default_t_ffrt_qos_utility,
            Qos::Default => ffrt_qos_default_t_ffrt_qos_default,
            Qos::UserInitiated => ffrt_qos_default_t_ffrt_qos_user_initiated,
        }
    }
}

impl From<ffrt_qos_default_t> for Qos {
    fn from(qos: ffrt_qos_default_t) -> Self {
        match qos {
            ffrt_qos_default_t_ffrt_qos_inherit => Qos::Inherit,
            ffrt_qos_default_t_ffrt_qos_background => Qos::Background,
            ffrt_qos_default_t_ffrt_qos_utility => Qos::Utility,
            ffrt_qos_default_t_ffrt_qos_default => Qos::Default,
            ffrt_qos_default_t_ffrt_qos_user_initiated => Qos::UserInitiated,
            _ => unreachable!(),
        }
    }
}
