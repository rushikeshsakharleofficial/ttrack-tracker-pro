#!/usr/bin/env python3
"""
Triggered on issue creation. Reads issue, sends codebase to NVIDIA NIM
(Qwen3 Coder 480B), applies the fix, creates branch + PR.
"""
import json, os, subprocess, sys, urllib.request, urllib.error, re

API_KEY      = os.environ.get("NVIDIA_API_KEY", "")
ISSUE_NUMBER = os.environ.get("ISSUE_NUMBER", "0")
ISSUE_TITLE  = os.environ.get("ISSUE_TITLE", "")
ISSUE_BODY   = os.environ.get("ISSUE_BODY", "")
GH_REPO      = os.environ.get("GITHUB_REPOSITORY", "")
MODEL        = "qwen/qwen3-coder-480b-a35b-instruct"

def run(cmd, **kw):
    return subprocess.run(cmd, check=True, text=True, capture_output=True, **kw)

def git(args, **kw):
    return run(["git"] + args, **kw)

# ── 1. Collect source files ───────────────────────────────────────────────────
EXTENSIONS = {".rs", ".toml", ".sh"}
files = {}
for root, dirs, fnames in os.walk("."):
    # skip hidden dirs and target/
    dirs[:] = [d for d in dirs if not d.startswith(".") and d != "target"]
    for fname in fnames:
        ext = os.path.splitext(fname)[1]
        if ext in EXTENSIONS:
            path = os.path.join(root, fname).lstrip("./")
            try:
                content = open(os.path.join(root, fname)).read()
                files[path] = content
            except Exception:
                pass

files_block = "\n\n".join(
    f"=== FILE: {p} ===\n{c}" for p, c in sorted(files.items())
)

# Trim to ~60K chars so we stay within context
if len(files_block) > 60000:
    files_block = files_block[:60000] + "\n... (truncated)"

# ── 2. Build prompt ───────────────────────────────────────────────────────────
prompt = f"""You are an expert Rust systems programmer. You must fix a bug reported as a GitHub issue.

IMPORTANT: Respond in English only. Be precise and concise.

## Issue #{ISSUE_NUMBER}: {ISSUE_TITLE}

{ISSUE_BODY}

## Codebase

{files_block}

## Instructions

Analyze the issue and produce a fix. Respond with ONLY a raw JSON object — no markdown, no code fences, no explanation outside the JSON.

The JSON must have exactly this shape:
{{
  "summary": "one sentence describing the fix",
  "files": [
    {{
      "path": "src/example.rs",
      "content": "...complete new file content..."
    }}
  ]
}}

Rules:
- "files" contains ONLY files you actually changed. Do not include unchanged files.
- "content" is the COMPLETE new content of the file (not a diff).
- If you cannot determine a safe fix, return {{"summary": "cannot fix safely", "files": []}}
- All text must be in English.
"""

# ── 3. Call NVIDIA NIM ────────────────────────────────────────────────────────
payload = json.dumps({
    "model": MODEL,
    "messages": [{"role": "user", "content": prompt}],
    "temperature": 0.1,
    "max_tokens": 8192,
    "top_p": 0.7,
}).encode()

req = urllib.request.Request(
    "https://integrate.api.nvidia.com/v1/chat/completions",
    data=payload,
    headers={
        "Authorization": f"Bearer {API_KEY}",
        "Content-Type": "application/json",
    },
)

print(f"Calling {MODEL} for issue #{ISSUE_NUMBER}...")
try:
    with urllib.request.urlopen(req, timeout=180) as resp:
        data = json.loads(resp.read())
    content = data["choices"][0]["message"]["content"].strip()
    print(f"Response: {len(content)} chars")
except urllib.error.HTTPError as e:
    body = e.read().decode(errors="replace")
    print(f"HTTP {e.code}: {body}", file=sys.stderr)
    sys.exit(1)
except Exception as e:
    print(f"API error: {e}", file=sys.stderr)
    sys.exit(1)

# Strip markdown fences if present
if content.startswith("```"):
    lines = content.splitlines()
    content = "\n".join(l for l in lines if not l.startswith("```")).strip()

# Extract JSON object
start = content.find("{")
end   = content.rfind("}") + 1
if start == -1 or end == 0:
    print("No JSON object in response", file=sys.stderr)
    print(content[:500], file=sys.stderr)
    sys.exit(1)

try:
    result = json.loads(content[start:end])
except Exception as e:
    print(f"JSON parse error: {e}", file=sys.stderr)
    print(content[start:start+500], file=sys.stderr)
    sys.exit(1)

summary    = result.get("summary", "AI fix")
file_edits = result.get("files", [])

if not file_edits:
    print(f"Model says: {summary}")
    print("No file changes produced — skipping branch/PR.")
    sys.exit(0)

print(f"Fix summary: {summary}")
print(f"Files to change: {[f['path'] for f in file_edits]}")

# ── 4. Create branch ──────────────────────────────────────────────────────────
slug = re.sub(r"[^a-z0-9]+", "-", ISSUE_TITLE.lower())[:40].strip("-")
branch = f"fix/issue-{ISSUE_NUMBER}-{slug}"

git(["config", "user.email", "ai-fixer@github-actions"])
git(["config", "user.name", "AI Issue Fixer"])
git(["checkout", "-b", branch])

# ── 5. Apply file changes ─────────────────────────────────────────────────────
changed = []
for edit in file_edits:
    path    = edit.get("path", "").strip()
    new_src = edit.get("content", "")
    if not path or not new_src:
        continue
    # Safety: no path traversal
    if ".." in path or path.startswith("/"):
        print(f"Skipping unsafe path: {path}", file=sys.stderr)
        continue
    os.makedirs(os.path.dirname(path) if os.path.dirname(path) else ".", exist_ok=True)
    with open(path, "w") as f:
        f.write(new_src)
    changed.append(path)
    print(f"  wrote {path}")

if not changed:
    print("No valid file changes — aborting.")
    sys.exit(0)

# ── 6. Verify it still compiles ───────────────────────────────────────────────
print("Checking compilation...")
check = subprocess.run(["cargo", "check", "--quiet"], capture_output=True, text=True)
if check.returncode != 0:
    print("Compilation failed — reverting changes.", file=sys.stderr)
    print(check.stderr[:2000], file=sys.stderr)
    git(["checkout", "HEAD", "--"] + changed)
    sys.exit(1)
print("Compilation OK.")

# ── 7. Commit ─────────────────────────────────────────────────────────────────
git(["add"] + changed)
commit_msg = (
    f"fix: {summary} (closes #{ISSUE_NUMBER})\n\n"
    f"Auto-generated by AI Issue Fixer using {MODEL} via NVIDIA NIM."
)
git(["commit", "-m", commit_msg])

# ── 8. Push branch ────────────────────────────────────────────────────────────
git(["push", "origin", branch])

# ── 9. Open PR ────────────────────────────────────────────────────────────────
pr_body = (
    f"## Auto-fix for #{ISSUE_NUMBER}\n\n"
    f"**Issue:** {ISSUE_TITLE}\n\n"
    f"**Fix summary:** {summary}\n\n"
    f"**Files changed:** {', '.join(f'`{p}`' for p in changed)}\n\n"
    f"**Model:** `{MODEL}` via NVIDIA NIM\n\n"
    f"Closes #{ISSUE_NUMBER}\n\n"
    "---\n"
    "*Auto-generated by AI Issue Fixer. Review carefully before merging.*"
)

result = subprocess.run(
    [
        "gh", "pr", "create",
        "--title", f"fix: {summary[:70]}",
        "--body", pr_body,
        "--base", "main",
        "--head", branch,
    ],
    capture_output=True, text=True
)
if result.returncode == 0:
    print(f"PR created: {result.stdout.strip()}")
else:
    print(f"PR creation failed: {result.stderr}", file=sys.stderr)
    sys.exit(1)
