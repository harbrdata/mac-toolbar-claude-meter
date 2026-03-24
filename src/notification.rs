use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::NSString;

// Ensure UserNotifications.framework is linked.
#[link(name = "UserNotifications", kind = "framework")]
unsafe extern "C" {}

/// Request notification authorization (call once at startup).
/// This prompts the user on first run; subsequent calls are no-ops.
pub fn request_authorization() {
    let Some(center) = notification_center() else {
        return;
    };
    // Request alert + sound permissions.
    // UNAuthorizationOptionAlert (1 << 2) | UNAuthorizationOptionSound (1 << 1) = 0x06
    let options: usize = 0x06;
    unsafe {
        let _: () = msg_send![&center, requestAuthorizationWithOptions: options, completionHandler: std::ptr::null::<AnyObject>()];
    }
}

/// Post a native macOS notification via UNUserNotificationCenter.
pub fn post(title: &str, subtitle: &str, body: &str) {
    let Some(center) = notification_center() else {
        eprintln!("UNUserNotificationCenter not available, falling back to osascript");
        post_osascript(title, subtitle, body);
        return;
    };

    let content = unsafe {
        let cls = AnyClass::get(c"UNMutableNotificationContent").unwrap();
        let content: Retained<AnyObject> = msg_send![cls, new];
        let _: () = msg_send![&content, setTitle: &*NSString::from_str(title)];
        let _: () = msg_send![&content, setSubtitle: &*NSString::from_str(subtitle)];
        let _: () = msg_send![&content, setBody: &*NSString::from_str(body)];
        content
    };

    unsafe {
        let cls = AnyClass::get(c"UNNotificationRequest").unwrap();
        let identifier = NSString::from_str("claude-meter-alert");
        let request: Retained<AnyObject> = msg_send![
            cls,
            requestWithIdentifier: &*identifier,
            content: &*content,
            trigger: std::ptr::null::<AnyObject>()
        ];
        let _: () = msg_send![
            &center,
            addNotificationRequest: &*request,
            withCompletionHandler: std::ptr::null::<AnyObject>()
        ];
    }
}

fn notification_center() -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"UNUserNotificationCenter")?;
    let center: Option<Retained<AnyObject>> = unsafe { msg_send![cls, currentNotificationCenter] };
    center
}

fn post_osascript(title: &str, subtitle: &str, body: &str) {
    let _ = std::process::Command::new("osascript")
        .args([
            "-e",
            &format!(
                "display notification \"{}\" with title \"{}\" subtitle \"{}\"",
                body, title, subtitle
            ),
        ])
        .spawn();
}
