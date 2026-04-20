//! Walk the UI Automation tree of the foreground window and print every
//! visible element with its control type, size, and accessible name.
//!
//! ```powershell
//! cargo run --example enumerate_foreground
//! ```
//!
//! The program counts down for 3 seconds before snapshotting, so you can
//! Alt-Tab to whichever window you want to inspect.

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use std::{thread, time::Duration};

    use uiautomation::{types::Handle, UIAutomation};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};

    println!("keyhop · foreground UI Automation walker");
    for s in (1..=3).rev() {
        println!("  switch to your target window... {s}");
        thread::sleep(Duration::from_secs(1));
    }

    // SAFETY: GetForegroundWindow has no preconditions; null is checked.
    // GetWindowTextW writes at most `buf.len()` UTF-16 code units.
    let (hwnd, title) = unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            anyhow::bail!("no foreground window");
        }
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut buf);
        let title = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
        (hwnd, title)
    };

    println!();
    println!("Foreground HWND : {:?}", hwnd.0);
    println!("Window title    : {title}");
    println!();

    let automation = UIAutomation::new()?;
    let root = automation.element_from_handle(Handle::from(hwnd.0 as isize))?;
    let walker = automation.get_control_view_walker()?;

    let mut count = 0usize;
    walk(&walker, &root, 0, &mut count)?;

    println!();
    println!("Visited {count} elements (depth-limited).");
    Ok(())
}

#[cfg(windows)]
fn walk(
    walker: &uiautomation::UITreeWalker,
    el: &uiautomation::UIElement,
    depth: usize,
    count: &mut usize,
) -> anyhow::Result<()> {
    const MAX_DEPTH: usize = 8;

    *count += 1;
    let role = el
        .get_control_type()
        .map(|c| format!("{c:?}"))
        .unwrap_or_else(|_| "?".to_string());
    let name = el.get_name().unwrap_or_default();
    let bounds = el.get_bounding_rectangle().ok();

    let indent = "  ".repeat(depth);
    if let Some(r) = bounds {
        println!(
            "{indent}{role:<14} {w:>4}x{h:<4}  '{name}'",
            w = r.get_width(),
            h = r.get_height(),
        );
    } else {
        println!("{indent}{role:<14} (no bounds)  '{name}'");
    }

    if depth >= MAX_DEPTH {
        return Ok(());
    }
    if let Ok(first) = walker.get_first_child(el) {
        let mut current = first;
        loop {
            walk(walker, &current, depth + 1, count)?;
            match walker.get_next_sibling(&current) {
                Ok(next) => current = next,
                Err(_) => break,
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This example only runs on Windows.");
}
