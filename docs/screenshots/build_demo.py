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


def assistant(blocks, age, end_turn=False):
    message = {"role": "assistant", "content": blocks}
    if end_turn:
        message["stop_reason"] = "end_turn"
    return {"type": "assistant", "timestamp": iso(age), "message": message}


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
    return thread(n, directory, title_kind, title, [(prompt, reply)], age)


def thread(n, directory, title_kind, title, pairs, age):
    cwd = work / directory
    lines = []
    for index, (prompt, reply) in enumerate(pairs):
        turn_age = age + (len(pairs) - 1 - index) * 240
        lines.append(user(cwd, prompt, turn_age + 120))
        lines.append(assistant([text(reply)], turn_age + 60, end_turn=True))
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
claude["timeout"] = thread(3, "upload-client", "none", "", [
    ("Why does the chunk uploader time out on large files?", "The whole request shares one 10 second timeout. A 40 MB chunk on a slow link needs longer, so the timeout fires before the last bytes arrive."),
    ("Can we just raise it?", "Raising it globally hides real failures. I would set a timeout per chunk, scaled by the chunk size, and retry a chunk once when it times out."),
    ("What does a retry do to the server load?", "Each retry resends the whole chunk, so a chunk that keeps failing costs double the traffic at worst. A cap of 2 retries keeps the timeout budget bounded."),
    ("Make the per-chunk timeout configurable.", "Added `chunk_timeout` to the client config with a default of 30 seconds. It is passed to every request."),
    ("And log when a retry happens.", "Each retry now logs the chunk number and the timeout that was used, at warning level."),
    ("Thanks, that is enough.", "Done. The change is in `config.py` and `upload.py`."),
], 90000)
claude["paginate"] = simple(4, "billing-api", "ai", "Paginate invoices", "Paginate GET /invoices with a cursor and a default page size of 50.", "Working on it: adding a cursor parameter and a Link header.", 20)
claude["rounding"] = simple(5, "billing-api", "ai", "Handle currency rounding in totals", "Totals are off by a cent for some invoices. Fix the rounding.", "Line totals are rounded individually, which can drift from the invoice total.\n\nShould I round per line or per invoice?", 900)
claude["migrate"] = simple(6, "billing-api", "ai", "Migrate invoice table to new schema", "Write the migration for the new invoice schema.", "The migration adds the new columns and backfills them in batches.", 260000)
claude["guide"] = simple(7, "docs-site", "ai", "Rewrite the getting started guide", "Rewrite the getting started guide for the 2.0 CLI.", "I restructured it into install, first project and deploy sections.", 14400)
claude["links"] = simple(8, "docs-site", "ai", "Fix broken links in the changelog", "Find and fix broken links in CHANGELOG.md.", "Fixed 6 links; two pointed at renamed pages.", 400000)
claude["debounce"] = simple(9, "mobile-app", "ai", "Debounce the search input", "Debounce the search input by 300 ms.", "Added a debounce hook and used it in the search screen.", 1500)
claude["dark"] = simple(10, "mobile-app", "custom", "Dark mode colour tokens", "Add dark mode colour tokens.", "Tokens added for background, surface and text.", 86400)
claude["logs"] = simple(11, "infra-scripts", "ai", "Rotate log files nightly", "Rotate the application logs nightly and keep 14 days.", "Added a logrotate config and a cron entry.", 172800)
STALE_MINUTES = 47
claude["dryrun"] = simple(12, "infra-scripts", "ai", "Add dry-run flag to deploy script", "Add a --dry-run flag to deploy.sh.", "The flag prints each step instead of running it. The health check step gets one retry and the dry run shows both attempts.", STALE_MINUTES * 60)
claude["backfill"] = thread(13, "billing-api", "ai", "Backfill archive", [
    ("Backfill the invoice archive for 2024.", "Backfilling in batches of 500 invoices. Each batch is idempotent, so a failed batch can retry without duplicates."),
    ("Use the retry helper from the upload client.", "Wired in. A batch now retries twice with backoff before it is marked failed."),
], 100)
claude["webhook"] = simple(14, "billing-api", "ai", "Retry failed webhook deliveries", "Retry failed webhook deliveries with a backoff.", "Failed deliveries are retried up to 5 times, one minute apart, then moved to a dead-letter list.", 100000)
claude["onboarding"] = simple(15, "mobile-app", "ai", "Ship the onboarding redesign", "Finish the onboarding redesign.", "The new screens are in. Two decisions are yours; they are listed in the definition of done.", 2400)
claude["waiting"] = simple(16, "docs-site", "ai", "Publish the 2.0 migration guide", "Publish the 2.0 migration guide.", "The guide is ready and waits on two reviews before it can go out.", 9000)
claude["offline"] = simple(17, "mobile-app", "ai", "Add offline mode to the sync screen", "Add offline mode to the sync screen.", "Edits are queued while offline and replayed on reconnect. Conflict handling and the docs are still to do.", 4000)
signing_cwd = work / "infra-scripts"
claude["signing"] = write_claude(18, "infra-scripts", "ai", "Rotate the signing keys", [
    user(signing_cwd, "Rotate the signing keys and update the deploy config.", 600),
    assistant([text("The new key pair is generated. Applying the change now."), tool("t1", "Bash", {"command": "terraform apply -auto-approve", "description": "Apply the key rotation"})], 300),
], 300)

for directory in DIRECTORIES:
    (work / directory).mkdir(parents=True, exist_ok=True)
(work / "billing-api" / ".git").mkdir(exist_ok=True)


def subagents(key, directory, agents):
    folder = project_dir(directory) / claude[key][0] / "subagents"
    folder.mkdir(parents=True, exist_ok=True)
    for name, description, quiet_minutes in agents:
        (folder / f"agent-{name}.meta.json").write_text(json.dumps({"agentType": "explore", "description": description}))
        transcript = folder / f"agent-{name}.jsonl"
        transcript.write_text('{"type":"user"}\n')
        stamp = NOW - quiet_minutes * 60
        os.utime(transcript, (stamp, stamp))


subagents("paginate", "billing-api", [("a1", "find callers of list_invoices", 0), ("a2", "check pagination in the client SDK", 0)])
subagents("backfill", "billing-api", [("b1", "count invoices per year", 0), ("b2", "verify archive checksums", 25)])

dod_dir = claude_dir / "dod"
dod_dir.mkdir(parents=True, exist_ok=True)
DOD_FILES = {
    "onboarding": """# Definition of done: onboarding redesign

## Core
- [x] new screens implemented
- [x] snapshot tests updated

## Amendment 2
- [x] copy reviewed
- [>] approve the final copy — owner: you — action: reply ok or send edits — why not mine: product decision
- [>] pick the launch date — owner: you — action: choose one of the two dates — why not mine: depends on your calendar
""",
    "waiting": """# Definition of done: 2.0 migration guide

## Core
- [x] guide drafted
- [x] examples run against 2.0

## Amendment 1
- [>] review of the new auth section — owner: security-team — waiting on security review — action: none until they answer
- [>] review of the new screenshots — owner: design-team — waiting on design sign-off — action: none until they answer
""",
    "offline": """# Definition of done: offline mode

## Core
- [x] edit queue implemented

## Amendment 1
- [x] replay on reconnect
- [ ] conflict resolution test
- [ ] document the offline limits
""",
    "checksum": """# Definition of done: flaky checksum test

## Core
- [x] cause found
- [x] each test uses its own temporary directory
- [x] 50 consecutive runs pass
- [>x] confirm the fix — owner: you — action: read the diff — why not mine: your review
""",
}
for key, content in DOD_FILES.items():
    (dod_dir / f"{time.strftime('%Y-%m-%d')}-{claude[key][0]}.md").write_text(content)

registry.mkdir(parents=True, exist_ok=True)
pids = []
live = [
    ("paginate", "busy", "billing-api", 0),
    ("backfill", "busy", "billing-api", 0),
    ("signing", "waiting", "infra-scripts", 0),
    ("rounding", "idle", "billing-api", 0),
    ("onboarding", "idle", "mobile-app", 0),
    ("waiting", "idle", "docs-site", 0),
    ("offline", "idle", "mobile-app", 0),
    ("checksum", "idle", "upload-client", 0),
    ("guide", "idle", "docs-site", 0),
    ("debounce", "idle", "mobile-app", 0),
    ("dryrun", "busy", "infra-scripts", STALE_MINUTES * 60),
]
for key, status, directory, status_age in live:
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
        "updatedAt": NOW_MS - status_age * 1000,
        "statusUpdatedAt": NOW_MS - status_age * 1000,
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


rollout("upload-client", "c0de0001-0000-4000-8000-000000000001", "Profile the checksum hot loop", "Most of the time is spent hashing 4 KiB blocks; a larger buffer halves it. A failed read gets one retry.", 5000)
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
(config_dir / "favorites.json").write_text(json.dumps([claude["retry"][0], claude["logs"][0], claude["paginate"][0]]))
(config_dir / "important.json").write_text(json.dumps([claude["rounding"][0]]))
(config_dir / "on_hold.json").write_text(json.dumps({claude["guide"][0]: {"since_ms": NOW_MS - 600_000}}))
(config_dir / "colors.json").write_text(json.dumps({claude["paginate"][0]: 0, claude["rounding"][0]: 6, claude["debounce"][0]: 3, claude["backfill"][0]: 9}))
(config_dir / "view.json").write_text(json.dumps({"level": "chat"}))
(config_dir / "ui.json").write_text(json.dumps({"split_ratio": 0.4, "mouse": False, "handoff_owner": "you"}))
