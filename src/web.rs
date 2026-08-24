//! Extras specific to the browser hosts (WebAudio and AudioWorklet).
//!
//! Browsers do not expose microphone selection through a synchronous device
//! enumeration API, so the regular [`HostTrait::devices`] path cannot carry
//! it: the list comes from the async `navigator.mediaDevices.enumerateDevices()`
//! and devices are picked by passing a `deviceId` constraint to
//! `getUserMedia`. This module bridges that gap with a process-wide
//! preference that the browser hosts consult when they acquire the
//! microphone.
//!
//! [`HostTrait::devices`]: crate::traits::HostTrait::devices

use std::cell::RefCell;

thread_local! {
    static PREFERRED_INPUT_DEVICE_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Select the microphone used by input streams built after this call.
///
/// `id` is a `MediaDeviceInfo.deviceId` obtained from JavaScript via
/// `navigator.mediaDevices.enumerateDevices()`; `None` restores the browser
/// default. The id is applied as an `exact` `deviceId` constraint on the
/// `getUserMedia` call, so a stale id fails stream setup (through the
/// stream's error callback) instead of silently capturing another device.
///
/// Already-built streams are unaffected; rebuild the input stream to switch.
pub fn set_preferred_input_device_id(id: Option<String>) {
    PREFERRED_INPUT_DEVICE_ID.with(|cell| *cell.borrow_mut() = id);
}

/// The id set by [`set_preferred_input_device_id`], if any.
pub(crate) fn preferred_input_device_id() -> Option<String> {
    PREFERRED_INPUT_DEVICE_ID.with(|cell| cell.borrow().clone())
}
