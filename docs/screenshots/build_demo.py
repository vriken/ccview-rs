#!/usr/bin/env python3
import json
from functools import partial
import os
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

json.dumps = partial(json.dumps, separators=(",", ":"))

root = Path(sys.argv[1]).resolve()
home = root / "home"
work = root / "work"
config = root / "config"
cache = root / "cache"
data = root / "data"
codex_home = root / "codex"
claude_dir = home / ".claude"
projects = claude_dir / "projects"
registry = claude_dir / "sessions"
pidfile = root / "pids"

NOW = time.time()
NOW_MS = int(NOW * 1000)

DIRECTORIES = ["upload-client", "billing-api", "docs-site", "mobile-app", "infra-scripts"]


def iso(offset_seconds):
    return time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime(NOW - offset_seconds))


def project_dir(directory):
    return projects / str(work / directory).replace("/", "-")


def uid(n):
    return f"{n:08x}-0000-4000-8000-{n:012x}"


def user(cwd, text, age):
    return {"type": "user", "cwd": str(cwd), "timestamp": iso(age), "message": {"role": "user", "content": text}}


def assistant(blocks, age):
    return {"type": "assistant", "timestamp": iso(age), "message": {"role": "assistant", "content": blocks}}


def text(value):
    return {"type": "text", "text": value}


def tool(tool_id, name, tool_input):
    return {"type": "tool_use", "id": tool_id, "name": name, "input": tool_input}


def tool_result(tool_id, output, age):
    return {
        "type": "user",
        "timestamp": iso(age),
        "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tool_id, "content": output}]},
    }


def write_claude(n, directory, title_kind, title, lines, age_seconds):
    cwd = work / directory
    cwd.mkdir(parents=True, exist_ok=True)
    folder = project_dir(directory)
    folder.mkdir(parents=True, exist_ok=True)
    session_id = uid(n)
    extra = []
    if title_kind == "ai":
        extra = [{"type": "ai-title", "aiTitle": title}]
    elif title_kind == "custom":
        extra = [{"type": "custom-title", "customTitle": title, "sessionId": session_id}]
    path = folder / f"{session_id}.jsonl"
    path.write_text("".join(json.dumps(line) + "\n" for line in lines + extra))
    stamp = NOW - age_seconds
    os.utime(path, (stamp, stamp))
    return session_id, cwd


def simple(n, directory, title_kind, title, prompt, reply, age):
    cwd = work / directory
    lines = [user(cwd, prompt, age + 120), assistant([text(reply)], age + 60)]
    return write_claude(n, directory, title_kind, title, lines, age)


def showcase(n, age):
    cwd = work / "upload-client"
    edit_one = {
        "file_path": "src/upload.py",
        "old_string": "def send_chunk(session, chunk):\n    response = session.put(chunk.url, data=chunk.data)\n    response.raise_for_status()\n    return response\n",
        "new_string": "def send_chunk(session, chunk, retries=3):\n    for attempt in range(retries):\n        response = session.put(chunk.url, data=chunk.data)\n        if response.status_code < 500:\n            response.raise_for_status()\n            return response\n        time.sleep(backoff(attempt))\n    response.raise_for_status()\n    return response\n",
    }
    reply = (
        "I added retries to `send_chunk` with exponential backoff.\n\n"
        "## What changed\n\n"
        "- `send_chunk` retries up to **3 times** on `5xx` responses\n"
        "- client errors (`4xx`) still fail immediately\n"
        "- the delay comes from a small `backoff` helper\n\n"
        "```python\ndef backoff(attempt, base=0.5, cap=8.0):\n    return min(cap, base * (2 ** attempt))\n```\n\n"
        "All 14 tests pass. I did not add jitter; say if you want it."
    )
    lines = [
        user(cwd, "Add retry logic to the upload client. Retry only on server errors, with exponential backoff.", age + 900),
        assistant([text("I'll start by looking at how chunks are sent."), tool("t1", "Read", {"file_path": "src/upload.py"})], age + 880),
        tool_result("t1", "def send_chunk(session, chunk):\n    response = session.put(chunk.url, data=chunk.data)\n    response.raise_for_status()\n    return response\n", age + 879),
        assistant([text("`send_chunk` has no retry at all. I'll wrap the request in a loop."), tool("t2", "Edit", edit_one)], age + 800),
        tool_result("t2", "The file src/upload.py has been updated.", age + 799),
        assistant([text("Now the tests."), tool("t3", "Bash", {"command": "pytest -q", "description": "Run the test suite"})], age + 400),
        tool_result("t3", "..............\n14 passed in 0.82s", age + 395),
        user(cwd, "Nice. Summarise what you changed.", age + 200),
        assistant([text(reply)], age + 120),
    ]
    return write_claude(n, "upload-client", "ai", "Add retry logic to the upload client", lines, age)


claude = {}
claude["retry"] = showcase(1, 3600)
claude["checksum"] = simple(2, "upload-client", "custom", "Fix flaky checksum test", "The checksum test fails about one run in ten. Find out why.", "The test compares against a file written by another test; I made each test use its own temporary directory.", 7200)
claude["timeout"] = simple(3, "upload-client", "none", "", "Why does the chunk uploader time out on large files?", "The default timeout is 10 seconds for the whole request; large chunks need a per-chunk timeout.", 90000)
claude["paginate"] = simple(4, "billing-api", "ai", "Paginate the invoices endpoint", "Paginate GET /invoices with a cursor and a default page size of 50.", "Working on it: adding a cursor parameter and a Link header.", 20)
claude["rounding"] = simple(5, "billing-api", "ai", "Handle currency rounding in totals", "Totals are off by a cent for some invoices. Fix the rounding.", "Line totals are rounded individually; I need to know whether to round per line or per invoice.", 150)
claude["migrate"] = simple(6, "billing-api", "ai", "Migrate invoice table to new schema", "Write the migration for the new invoice schema.", "The migration adds the new columns and backfills them in batches.", 260000)
claude["guide"] = simple(7, "docs-site", "ai", "Rewrite the getting started guide", "Rewrite the getting started guide for the 2.0 CLI.", "I restructured it into install, first project and deploy sections.", 14400)
claude["links"] = simple(8, "docs-site", "ai", "Fix broken links in the changelog", "Find and fix broken links in CHANGELOG.md.", "Fixed 6 links; two pointed at renamed pages.", 400000)
claude["debounce"] = simple(9, "mobile-app", "ai", "Debounce the search input", "Debounce the search input by 300 ms.", "Added a debounce hook and used it in the search screen.", 600)
claude["dark"] = simple(10, "mobile-app", "custom", "Dark mode colour tokens", "Add dark mode colour tokens.", "Tokens added for background, surface and text.", 86400)
claude["logs"] = simple(11, "infra-scripts", "ai", "Rotate log files nightly", "Rotate the application logs nightly and keep 14 days.", "Added a logrotate config and a cron entry.", 172800)
claude["dryrun"] = simple(12, "infra-scripts", "ai", "Add dry-run flag to deploy script", "Add a --dry-run flag to deploy.sh.", "The flag prints each step instead of running it.", 30000)

for directory in DIRECTORIES:
    (work / directory).mkdir(parents=True, exist_ok=True)

subagent_dir = project_dir("billing-api") / uid(4) / "subagents"
subagent_dir.mkdir(parents=True, exist_ok=True)
for name, description in [("a1", "find callers of list_invoices"), ("a2", "check pagination in the client SDK")]:
    (subagent_dir / f"agent-{name}.meta.json").write_text(json.dumps({"agentType": "explore", "description": description}))
    (subagent_dir / f"agent-{name}.jsonl").write_text('{"type":"user"}\n')

registry.mkdir(parents=True, exist_ok=True)
pids = []
live = [("paginate", "busy", "billing-api"), ("rounding", "waiting", "billing-api"), ("debounce", "idle", "mobile-app")]
for key, status, directory in live:
    process = subprocess.Popen(["sleep", "3600"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    pids.append(process.pid)
    session_id = claude[key][0]
    entry = {
        "pid": process.pid,
        "sessionId": session_id,
        "cwd": str(work / directory),
        "status": status,
        "kind": "interactive",
        "startedAt": NOW_MS - 3_600_000,
        "updatedAt": NOW_MS,
        "statusUpdatedAt": NOW_MS,
    }
    (registry / f"{process.pid}-{session_id}.json").write_text(json.dumps(entry))
pidfile.write_text("\n".join(str(pid) for pid in pids) + "\n")


def rollout(directory, session_id, prompt, reply, age, name=None):
    cwd = work / directory
    day = time.strftime("%Y/%m/%d", time.gmtime(NOW - age))
    folder = codex_home / "sessions" / day
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"rollout-{time.strftime('%Y-%m-%dT%H-%M-%S', time.gmtime(NOW - age))}-{session_id}.jsonl"
    records = [
        {"timestamp": iso(age + 60), "type": "session_meta", "payload": {"id": session_id, "cwd": str(cwd)}},
        {"timestamp": iso(age + 50), "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": prompt}]}},
        {"timestamp": iso(age + 40), "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": reply}]}},
    ]
    path.write_text("".join(json.dumps(record) + "\n" for record in records))
    stamp = NOW - age
    os.utime(path, (stamp, stamp))
    if name:
        with open(codex_home / "session_index.jsonl", "a") as index:
            index.write(json.dumps({"id": session_id, "thread_name": name}) + "\n")


rollout("upload-client", "c0de0001-0000-4000-8000-000000000001", "Profile the checksum hot loop", "Most of the time is spent hashing 4 KiB blocks; a larger buffer halves it.", 5000)
rollout("billing-api", "c0de0002-0000-4000-8000-000000000002", "Summarise what the weekly report job does", "It reads last week's invoices, groups them by customer and writes a CSV.", 43000, name="Explain the weekly report job")

(data / "opencode").mkdir(parents=True, exist_ok=True)
database = data / "opencode" / "opencode.db"
if database.exists():
    database.unlink()
connection = sqlite3.connect(database)
connection.executescript(
    """
    CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT NOT NULL, parent_id TEXT, title TEXT NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER);
    CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
    CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
    """
)


def opencode_session(session_id, directory, title, prompt, reply, age):
    created = NOW_MS - age * 1000
    connection.execute("INSERT INTO session VALUES (?,?,?,?,?,?)", (session_id, str(work / directory), None, title, created, None))
    for index, (role, body) in enumerate([("user", prompt), ("assistant", reply)]):
        message_id = f"msg_{session_id}_{index}"
        connection.execute("INSERT INTO message VALUES (?,?,?,?,?)", (message_id, session_id, created + index, created + index, json.dumps({"role": role, "time": {"created": created + index}})))
        connection.execute("INSERT INTO part VALUES (?,?,?,?,?,?)", (f"prt_{session_id}_{index}", message_id, session_id, created + index, created + index, json.dumps({"type": "text", "text": body})))


opencode_session("ses_demo1", "docs-site", "Add a search page to the docs", "Add a search page to the docs site.", "I added a search route backed by a prebuilt index.", 50000)
opencode_session("ses_demo2", "mobile-app", "Fix the onboarding crash", "The onboarding screen crashes on first launch.", "A null avatar URL was dereferenced; added a guard.", 120000)
connection.commit()
connection.close()

config_dir = config / "ccview-rs"
config_dir.mkdir(parents=True, exist_ok=True)
(cache).mkdir(parents=True, exist_ok=True)
(config_dir / "favorites.json").write_text(json.dumps([claude["retry"][0], claude["guide"][0], claude["paginate"][0]]))
(config_dir / "hidden.json").write_text(json.dumps([claude["links"][0]]))
(config_dir / "colors.json").write_text(json.dumps({claude["paginate"][0]: 0, claude["rounding"][0]: 6, claude["debounce"][0]: 3}))
(config_dir / "view.json").write_text(json.dumps({"level": "chat"}))
(config_dir / "ui.json").write_text(json.dumps({"split_ratio": 0.4, "mouse": False}))
