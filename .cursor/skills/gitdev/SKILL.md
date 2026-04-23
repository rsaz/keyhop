---
name: gitdev
description: Generate a structured development plan for a GitHub issue. Use when the user mentions "gitdev" followed by an issue number, or when they want a development plan, implementation strategy, or technical breakdown for a GitHub issue.
---

# Development Plan Generator (gitdev)

Generates a detailed, actionable development plan for implementing a GitHub issue. The plan is presented in chat for discussion and iteration with the developer.

## Usage

```
gitdev <issue-number>
gitdev 42
```

## Workflow

### 1. Fetch the Issue

Use the GitHub MCP `issue_read` tool to retrieve the issue and any requirements:

```
server: user-github
toolName: issue_read
arguments:
  owner: rsaz
  repo: keyhop
  issue_number: <N>
  method: get
```

Also fetch **comments** to check for any verified requirements or technical discussion:

```
server: user-github
toolName: issue_read
arguments:
  owner: rsaz
  repo: keyhop
  issue_number: <N>
  method: get_comments
```

### 2. Analyze Context

Before generating the plan, understand:
- The **problem** being solved
- Any **requirements or constraints** (from `gitv` or other comments)
- **Current codebase state** (use Read/Grep/SemanticSearch to understand relevant code)
- **Similar patterns** in the codebase (how have similar features been implemented?)
- **Dependencies** or **affected systems**

### 3. Generate the Development Plan

Present a structured plan with these sections:

````markdown
# Development Plan: Issue #<N> - <title>

## Summary
[2-3 sentence overview of what will be built and why]

## Affected Files
- `path/to/file1.rs` — [what changes here]
- `path/to/file2.rs` — [what changes here]
- `path/to/file3.rs` — [new file, what it does]

## Implementation Steps

### Step 1: [Descriptive name]
**What:** [What you're doing in this step]
**Where:** `path/to/file.rs` (lines XXX-YYY if modifying existing code)
**How:**
```rust
// Pseudocode or actual code snippet showing the change
pub fn new_function() -> Result<()> {
    // ...
}
```
**Why:** [Rationale for this approach]

### Step 2: [Next step]
...

## Testing Strategy

### Unit Tests
- `test_scenario_1` — [what it verifies]
- `test_scenario_2` — [what it verifies]

### Integration Tests
[If applicable]

### Manual Testing
1. Step 1 to reproduce
2. Step 2 to verify
3. Expected outcome

## Edge Cases to Handle
- Edge case 1 → how to handle it
- Edge case 2 → how to handle it

## Risks / Considerations
- **Risk 1:** [description] → Mitigation: [how to address]
- **Risk 2:** [description] → Mitigation: [how to address]

## Dependencies
- [ ] Dependency 1 (if any external crates, APIs, or other issues)
- [ ] Dependency 2

## Estimated Complexity
**Complexity:** [Low | Medium | High]
**Reasoning:** [Why this complexity estimate]

---
_Generated from issue #<N>. This is a living plan — discuss and refine as needed._
````

### 4. Present for Discussion

After presenting the plan, invite the developer to:
- Ask questions about any step
- Request more detail on specific areas
- Suggest alternative approaches
- Identify gaps or concerns

**Be conversational.** The plan is a starting point, not a rigid spec.

## Plan Quality Guidelines

### Make It Actionable
- Steps should be concrete and in logical order
- File paths should be actual paths in the keyhop codebase
- Code snippets should show the real API or pattern (not abstract pseudocode)

### Be Realistic About Complexity
- Don't hide complexity — if a step is tricky, say so and why
- Call out areas where the developer might need to experiment
- Note any parts that might require multiple attempts to get right

### Reference Existing Patterns
When suggesting an implementation approach, **reference similar code** already in the repo:

```markdown
**Pattern to follow:** See how `window_picker.rs` enumerates and filters windows
(lines 45-120) — apply the same filtering logic here.
```

This helps the developer understand the conventions and copy working patterns.

### Don't Over-Specify
- You're generating a plan, not the final code
- Leave room for the developer to make tactical decisions
- Focus on the "what" and "why," not every "how"

## Codebase Context

When generating plans for keyhop issues, be aware of:

- **Architecture:** Windows backend (`src/windows/`), core library (`src/lib.rs`), binary (`src/main.rs`)
- **UI Automation:** `uiautomation` crate for element picking
- **Hotkeys:** `global-hotkey` crate
- **Overlay:** Win32 layered windows (`src/windows/overlay.rs`)
- **Config:** TOML at `%APPDATA%\keyhop\config.toml`
- **Build system:** `cargo-wix` for MSI, `Scripts.toml` for dev commands

Use `SemanticSearch`, `Grep`, or `Read` to understand specific areas before generating the plan.

## Error Handling

- **Issue not found (404)**: Tell the user the issue doesn't exist.
- **MCP auth failure (403)**: Remind the user to authenticate the GitHub MCP if needed.
- **Unclear requirements**: If the issue is vague or hasn't been through `gitv`, note that in the plan and flag assumptions.

## Example Output (abbreviated)

```
# Development Plan: Issue #15 - Add Firefox support for element picker

## Summary
Extend the element picker to work with Firefox in addition to Chromium
browsers by implementing the Firefox-specific accessibility activation.

## Affected Files
- `src/windows/mod.rs` — add Firefox detection and activation
- `src/windows/hotkey.rs` — (no changes, for reference)

## Implementation Steps

### Step 1: Detect Firefox processes
**What:** Add Firefox to the browser detection logic
**Where:** `src/windows/mod.rs`, line 185 (within `is_browser` fn)
**How:**
```rust
fn is_browser(exe_name: &str) -> bool {
    matches!(
        exe_name,
        "chrome.exe" | "msedge.exe" | "firefox.exe" // <-- add this
    )
}
```
**Why:** Need to identify Firefox windows to apply different activation.

### Step 2: Implement Firefox accessibility activation
**What:** Send `WM_GETOBJECT` with Firefox-specific parameters
**Where:** `src/windows/mod.rs`, new function `activate_firefox_accessibility`
**How:**
```rust
// Firefox uses different IPC — see Mozilla docs on IAccessible2
fn activate_firefox_accessibility(hwnd: HWND) -> Result<()> {
    // Research: Mozilla's a11y IPC differs from Chromium
    // May need to enumerate child HWNDs with different class names
}
```
**Why:** Firefox doesn't respond to the same `WM_GETOBJECT` dance as
Chromium. Requires Mozilla-specific approach.

...

## Testing Strategy
### Manual Testing
1. Open Firefox, navigate to a page with links
2. Press `Ctrl+Shift+Space`
3. Verify hints appear on page content (not just chrome)

## Risks / Considerations
- **Risk:** Firefox's accessibility API is less documented than Chromium's
  → Mitigation: May need to experiment; check Firefox source or ask on
  Mozilla accessibility forums.

## Estimated Complexity
**Complexity:** Medium-High
**Reasoning:** Unfamiliar with Firefox's a11y activation; will likely need
trial-and-error.
```

## Notes

- The plan is **transient** — it exists only in the chat for discussion
- The user can then say "let's start with step 1" and you implement together
- Plans can be regenerated or refined as requirements change
