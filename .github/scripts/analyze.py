#!/usr/bin/env python3
"""Send git diff to NVIDIA NIM (Kimi K2) and write issues JSON to /tmp/issues.json."""
import json, os, sys, urllib.request, urllib.error

api_key = os.environ.get("NVIDIA_API_KEY", "")
if not api_key:
    print("NVIDIA_API_KEY not set", file=sys.stderr)
    json.dump([], open("/tmp/issues.json", "w"))
    sys.exit(0)

diff   = open("/tmp/diff.txt").read()
commit = open("/tmp/commit.txt").read().strip()
sha    = open("/tmp/sha.txt").read().strip()

if not diff.strip():
    print("Empty diff — nothing to analyze.")
    json.dump([], open("/tmp/issues.json", "w"))
    sys.exit(0)

prompt = (
    "You are a senior Rust code reviewer. "
    "IMPORTANT: Respond in English only.\n\n"
    "Analyze the following git diff and commit message. "
    "Find only real bugs, security vulnerabilities, and correctness problems. "
    "Do NOT report style nits, formatting issues, or minor suggestions.\n\n"
    f"Commit message: {commit}\n"
    f"Commit SHA: {sha}\n\n"
    f"Diff:\n{diff}\n\n"
    "Respond with ONLY a raw JSON array — absolutely no markdown, no code fences, "
    "no explanation text before or after. "
    "Each element must have exactly these keys:\n"
    '  "title"    : string, short issue title under 70 chars, in English\n'
    '  "body"     : string, detailed description in English with file:line if known, '
    "impact, and suggested fix\n"
    '  "labels"   : array with one string: "bug", "security", or "enhancement"\n'
    '  "severity" : string, one of "critical", "high", "medium"\n\n'
    "Return [] if no issues of medium severity or higher are found. "
    "Only report issues you are highly confident about."
)

payload = json.dumps({
    "model": "moonshotai/kimi-k2.6",
    "messages": [{"role": "user", "content": prompt}],
    "temperature": 0.1,
    "max_tokens": 2048,
    "top_p": 0.7,
}).encode()

req = urllib.request.Request(
    "https://integrate.api.nvidia.com/v1/chat/completions",
    data=payload,
    headers={
        "Authorization": f"Bearer {api_key}",
        "Content-Type": "application/json",
    },
)

try:
    with urllib.request.urlopen(req, timeout=90) as resp:
        data = json.loads(resp.read())
    content = data["choices"][0]["message"]["content"].strip()
    print(f"Model response length: {len(content)} chars")
except urllib.error.HTTPError as e:
    body = e.read().decode(errors="replace")
    print(f"HTTP {e.code}: {body}", file=sys.stderr)
    content = "[]"
except Exception as e:
    print(f"API error: {e}", file=sys.stderr)
    content = "[]"

# Strip markdown fences if model wrapped JSON anyway
if content.startswith("```"):
    lines = content.splitlines()
    content = "\n".join(l for l in lines if not l.startswith("```")).strip()

# Find the first [...] array in the response
start = content.find("[")
end   = content.rfind("]")
if start != -1 and end != -1 and end >= start:
    content = content[start:end+1]

try:
    issues = json.loads(content)
    if not isinstance(issues, list):
        issues = []
except Exception as e:
    print(f"JSON parse error: {e}\nContent was: {content[:500]}", file=sys.stderr)
    issues = []

print(f"Parsed {len(issues)} issue(s)")
json.dump(issues, open("/tmp/issues.json", "w"), indent=2)
