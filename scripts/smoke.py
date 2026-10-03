#!/usr/bin/env python3
"""Exercise real stdio JSON-RPC. Default is offline; --live-env is explicitly opt-in."""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading


def run(binary, env_file=None, modules="lms,mail"):
    with tempfile.TemporaryDirectory(prefix="campus-mcp-smoke-") as home:
        env = os.environ.copy()
        args = [str(Path(binary).resolve())]
        if env_file:
            args += ["--env-file", str(Path(env_file).resolve())]
        else:
            env["HOME"] = home
            for key in ["LMS_SERVICE", "PLMS_USERNAME", "PLMS_PASSWORD", "MAIL_TENANT"]:
                env.pop(key, None)
        args += ["serve", "--modules", modules, "--cache-dir", str(Path(home) / "cache")]
        lms = "lms" in modules.split(",")
        mail = "mail" in modules.split(",")
        tool_count = 2 + (6 if lms else 0) + (2 if mail else 0)
        # Keep all source content and stderr out of the test report.
        with tempfile.TemporaryFile(mode="w+") as errors:
            proc = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=errors, text=True, env=env)
            replies = queue.Queue()
            def consume():
                for line in proc.stdout:
                    try:
                        replies.put(json.loads(line))
                    except ValueError:
                        replies.put(RuntimeError("Non-JSON data on MCP stdout"))
                replies.put(RuntimeError("MCP stdout closed"))
            thread = threading.Thread(target=consume, daemon=True)
            thread.start()
            next_id = 0
            def send(method, params, notification=False):
                nonlocal next_id
                next_id += 1
                payload = {"jsonrpc": "2.0", "method": method, "params": params}
                if not notification:
                    payload["id"] = next_id
                proc.stdin.write(json.dumps(payload) + "\n")
                proc.stdin.flush()
                if notification:
                    return
                while True:
                    reply = replies.get(timeout=195 if env_file else 15)
                    if isinstance(reply, Exception):
                        raise reply
                    if reply.get("id") == next_id:
                        if "error" in reply:
                            raise RuntimeError(f"JSON-RPC error for {method}")
                        return reply["result"]
            def call(name, arguments):
                return send("tools/call", {"name": name, "arguments": arguments})
            try:
                init = send("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                            "clientInfo": {"name": "campus-smoke", "version": "1"}})
                assert init["serverInfo"]["name"] == "campus-mcp"
                send("notifications/initialized", {}, notification=True)
                assert len(send("tools/list", {})["tools"]) == tool_count
                status = call("campus_status", {})
                assert status["structuredContent"]["read_only"] is True
                invalid = [("attachment_read", {"id": "/etc/passwd"})]
                if lms:
                    invalid.append(("lms_read", {"url": "https://example.invalid/"}))
                if mail:
                    invalid.append(("mail_list", {"limit": 0}))
                for name, arguments in invalid:
                    assert call(name, arguments).get("isError") is True
                print(f"PASS: stdio handshake, {tool_count} tools, status, rejected inputs", flush=True)
                if env_file:
                    checks = []
                    if lms:
                        checks += [("lms_courses", {}, "courses"),
                                   ("lms_deadlines", {"limit": 1}, "deadlines"),
                                   ("lms_notices", {"limit": 1}, "notices"),
                                   ("lms_assignments", {"limit": 1}, "assignments")]
                    if mail:
                        checks.append(("mail_list", {"limit": 1}, "messages"))
                    def checked(name, arguments):
                        result = call(name, arguments)
                        if result.get("isError"):
                            message = result.get("structuredContent", {}).get("error", "request failed")
                            raise RuntimeError(f"{name}: {message}")
                        print(f"PASS: {name} (source contents withheld)", flush=True)
                        return result["structuredContent"]
                    for name, arguments, field in checks:
                        rows = checked(name, arguments)[field]
                        assert isinstance(rows, list)
                        if rows and name == "lms_notices":
                            checked("lms_read", {"url": rows[0]["url"]})
                        if rows and name == "lms_assignments":
                            checked("lms_submission", {"url": rows[0]["url"]})
                        if rows and name == "mail_list":
                            checked("mail_read", {"id": rows[0]["id"]})
                else:
                    for name in (["lms_courses"] if lms else []) + (["mail_list"] if mail else []):
                        assert call(name, {}).get("isError") is True
                    print("PASS: missing credentials are tool errors, no interactive login", flush=True)
            finally:
                proc.stdin.close()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.terminate()
                    proc.wait(timeout=5)
                thread.join(timeout=2)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", default="target/debug/campus-mcp")
    parser.add_argument("--live-env", help="Explicit opt-in to read real LMS/mail using this env file")
    parser.add_argument("--modules", choices=["lms", "mail", "lms,mail"], default="lms,mail")
    options = parser.parse_args()
    run(options.binary, options.live_env, options.modules)
