---
name: gitv
description: Verify and refine GitHub issue requirements through discussion. Use when the user mentions "gitv" followed by an issue number, or when they want to verify, discuss, or refine requirements for a GitHub issue before implementation.
---

# Issue Verification and Requirements Refinement (gitv)

Streamlines the process of understanding and documenting requirements for GitHub issues by facilitating a structured discussion and then updating the issue with formal requirements.

## Usage

```
gitv <issue-number>
gitv 42
```

## Workflow

### 1. Fetch the Issue

Use the GitHub MCP `issue_read` tool to retrieve issue details:

```
server: user-github
toolName: issue_read
arguments:
  owner: rsaz
  repo: keyhop
  issue_number: <N>
  method: get
```

### 2. Present and Discuss

Present the issue to the user:
- **Title** and **number**
- **Body** (the original description)
- **Current labels**, **assignees**, **state**
- Any existing **comments** (if relevant to understanding context)

Then engage in a **conversational discussion** to:
- Clarify ambiguities
- Understand the "why" behind the request
- Identify edge cases or constraints
- Confirm technical approach
- Surface dependencies or risks

**Ask focused questions** like:
- "Should this work for all Chromium browsers or just Chrome/Edge?"
- "What should happen if the config file is malformed?"
- "Do you want this in v0.3.0 or v0.4.0?"

Continue the discussion until the user confirms: **"yes, we have a good understanding"** or similar affirmation.

### 3. Create Requirements Document

Once confirmed, draft a **formal requirements comment** using this structure:

```markdown
## Requirements (verified)

### Problem Statement
[1-2 sentences: what problem does this solve?]

### Acceptance Criteria
- [ ] Criterion 1 (specific, testable)
- [ ] Criterion 2
- [ ] Criterion 3

### Technical Approach
[High-level implementation strategy, if discussed]

### Edge Cases / Constraints
- Edge case 1
- Edge case 2

### Out of Scope
- Thing we explicitly decided not to include
- Thing deferred to a future version

### Testing Plan
- Unit tests: [what to test]
- Integration tests: [if applicable]
- Manual verification: [steps]

---
_Requirements verified via discussion on [date]._
```

**Adapt the structure** based on the issue — not every section will always apply. Focus on **clarity and specificity**.

### 4. Update the Issue on GitHub

Use the GitHub MCP tools to:

1. **Post the requirements comment**:
   ```
   server: user-github
   toolName: add_issue_comment
   arguments:
     owner: rsaz
     repo: keyhop
     issue_number: <N>
     body: <requirements-doc>
   ```

2. **Update labels** (if not already present):
   ```
   server: user-github
   toolName: issue_write
   arguments:
     owner: rsaz
     repo: keyhop
     issue_number: <N>
     method: update
     labels: ["verified", "ready"]
   ```

3. **Assign to the user** (if requested or if unassigned):
   ```
   server: user-github
   toolName: issue_write
   arguments:
     owner: rsaz
     repo: keyhop
     issue_number: <N>
     method: update
     assignees: ["rsaz"]
   ```

### 5. Confirm Completion

After successfully updating the issue, provide a summary:

```
✅ Issue #<N> updated:
   - Requirements documented: <comment-url>
   - Labels: verified, ready
   - Assigned to: rsaz
   
Ready for implementation. Use `gitdev <N>` to generate a development plan.
```

## Error Handling

- **Issue not found (404)**: Tell the user the issue doesn't exist and suggest double-checking the number.
- **MCP auth failure (403)**: Remind the user to authenticate the GitHub MCP if needed (check for `mcp_auth` tool).
- **Ambiguous discussion**: If the user's answers are still unclear after 2-3 rounds, summarize what you understand and what's still ambiguous, then ask for explicit confirmation or clarification.

## Notes

- This skill is **conversational** — don't rush to post the comment. The value is in the discussion and clarity.
- Requirements should be **specific and testable** — avoid vague criteria like "works well" or "is fast."
- The user may ask follow-up questions after you post the comment — that's expected. Update the comment if needed.
