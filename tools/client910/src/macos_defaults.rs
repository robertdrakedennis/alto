//! Process-local AppKit defaults the client registers before its window
//! opens (macOS only).
//!
//! `NSAutoFillHeuristicsEnabled = NO`: AppKit's password AutoFill treats
//! winit's view (an `NSTextInputClient`) as a text field. When it becomes
//! first responder, `NSAutoFillHeuristicController` sets up a completion-list
//! remote view through the XPC service `com.apple.SafariPlatformSupport.Helper`
//! (traced with lldb: `-[SPSafariPlatformSupport
//! _setUpCompletionListViewControllerWithSetUpCompletionListViewController:]`
//! from `_showOrHideAutoFillForCurrentTextInputContextIfAppropriate`). That
//! helper outlives the client, so every launch left one resident process
//! behind: 634 of them after a day of agent runs, about 4 MB each plus their
//! kernel (wired) memory. A game view never wants AutoFill. The value goes in
//! the registration domain (in memory, nothing written to disk), so a user
//! who sets the default explicitly still wins.

/// Registers the defaults (once, at start-up, before the event loop).
pub fn register() {
    #[cfg(target_os = "macos")]
    objc2::rc::autoreleasepool(|_| {
        use objc2::runtime::{AnyObject, Bool};
        use objc2::{class, msg_send};
        // SAFETY: plain Foundation class messages with valid arguments; the
        // returned objects are autoreleased into the surrounding pool.
        unsafe {
            let key: *mut AnyObject = msg_send![
                class!(NSString),
                stringWithUTF8String: c"NSAutoFillHeuristicsEnabled".as_ptr()
            ];
            let no: *mut AnyObject = msg_send![class!(NSNumber), numberWithBool: Bool::NO];
            let defaults: *mut AnyObject =
                msg_send![class!(NSDictionary), dictionaryWithObject: no, forKey: key];
            let user: *mut AnyObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
            let _: () = msg_send![user, registerDefaults: defaults];
        }
    });
}
