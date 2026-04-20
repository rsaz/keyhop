//! Minimal Win32 sanity check: print the title and handle of the current
//! foreground window. Run with:
//!
//! ```powershell
//! cargo run -p keyhop-windows --example enumerate_foreground
//! ```
//!
//! Once this works, the next milestone is replacing the placeholder body with
//! a UI Automation tree walk that prints every interactable child element.

#[cfg(windows)]
fn main() {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};

    // SAFETY: both calls are simple Win32 functions with no preconditions.
    // GetWindowTextW writes at most `buf.len()` UTF-16 code units.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            eprintln!("No foreground window.");
            return;
        }

        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut buf);
        let title = String::from_utf16_lossy(&buf[..len.max(0) as usize]);

        println!("Foreground HWND : {:?}", hwnd.0);
        println!("Window title    : {}", title);
        println!();
        println!("Next step: replace this with a UI Automation tree walk that");
        println!("collects elements supporting the Invoke pattern.");
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This example only runs on Windows.");
}
