#!/usr/bin/env python3
"""Synthetic stdio peer: no network, project reads, credentials or vendor tools."""
import json
import os
from pathlib import Path
import sys
import time

mode, trace_path, pid_path = sys.argv[1:]
Path(pid_path).write_text(str(os.getpid()))
held = []
cancelled = []
documents = {}
behavior = {"empty": True} if mode == "empty-primary" else {}


def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line == b"\r\n":
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":", 1)[1].strip())
    body = sys.stdin.buffer.read(length)
    value = json.loads(body)
    with open(trace_path, "a", encoding="utf-8") as trace:
        trace.write(json.dumps(value, ensure_ascii=False) + "\n")
    return value


def send(value):
    body = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()
    frame = f"Content-Length: {len(body)}\r\n\r\n".encode() + body
    if mode == "fragment":
        for index in range(0, len(frame), 3):
            sys.stdout.buffer.write(frame[index:index + 3])
            sys.stdout.buffer.flush()
    else:
        sys.stdout.buffer.write(frame)
        sys.stdout.buffer.flush()


def result(identifier, value):
    send({"jsonrpc": "2.0", "id": identifier, "result": value})


def error(identifier, code, message):
    send({"jsonrpc": "2.0", "id": identifier, "error": {"code": code, "message": message}})


if mode == "crash-start":
    sys.stderr.write("fixture startup crash\n")
    sys.stderr.flush()
    sys.exit(23)

while True:
    message = read()
    if message is None:
        break
    method = message.get("method")
    params = message.get("params", {})
    identifier = message.get("id")
    if method in ("textDocument/definition", "textDocument/references", "textDocument/completion"):
        if mode == "crash-query":
            sys.stderr.write("fixture query crash\n")
            sys.stderr.flush()
            sys.exit(9)
        if behavior.get("delay"):
            time.sleep(behavior["delay"])
        if behavior.get("error"):
            error(identifier, -32603, "fixture primary unavailable")
            continue
        if behavior.get("empty"):
            result(identifier, [] if method == "textDocument/references" else None)
            continue
        if method in behavior:
            result(identifier, behavior[method])
            continue
    if method == "initialize":
        if mode == "no-initialize":
            continue
        if mode == "init-error":
            error(identifier, -32002, "fixture startup denied")
            continue
        if mode == "bad-frame":
            sys.stdout.buffer.write(b"Content-Length: 999999999\r\n\r\n")
            sys.stdout.buffer.flush()
            continue
        send({"jsonrpc": "2.0", "id": "configuration-😀", "method": "workspace/configuration", "params": {"items": [{"section": "fixture.option"}, {"section": "missing"}]}})
        config_reply = read()
        assert config_reply["id"] == "configuration-😀" and config_reply["result"] == [42, None]
        send({"jsonrpc": "2.0", "id": -7, "method": "fixture/unknownClientRequest", "params": {}})
        unknown_reply = read()
        assert unknown_reply["id"] == -7 and unknown_reply["error"]["code"] == -32601
        capabilities = {} if mode == "no-capabilities" else {"definitionProvider": True, "referencesProvider": True, "completionProvider": {"resolveProvider": False}, "textDocumentSync": {"openClose": True, "change": 1 if mode == "full" else 2, "save": {"includeText": True}}}
        capabilities["positionEncoding"] = "utf-8" if mode == "utf8" else "utf-32" if mode == "utf32" else "unsupported" if mode == "bad-encoding" else "utf-16"
        result(identifier, {"capabilities": capabilities, "serverInfo": {"name": "synthetic-peer", "version": "1"}})
    elif method == "initialized":
        if mode == "blocked-stdin":
            time.sleep(60)
        if mode == "stderr-flood":
            sys.stderr.write("x" * 200000 + "stderr end marker")
            sys.stderr.flush()
        send({"jsonrpc": "2.0", "method": "window/logMessage", "params": {"type": 3, "message": "naïve — 😀"}})
    elif method == "textDocument/didOpen":
        documents[params["textDocument"]["uri"]] = params["textDocument"]
    elif method == "textDocument/didClose":
        documents.pop(params["textDocument"]["uri"], None)
    elif method in ("textDocument/didChange", "textDocument/didSave"):
        pass
    elif method == "textDocument/definition":
        uri = params["textDocument"]["uri"]
        result(identifier, [{"targetUri": uri, "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 2, "character": 0}}, "targetSelectionRange": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 6}}}])
    elif method == "textDocument/references":
        assert params["context"]["includeDeclaration"]
        result(identifier, [{"uri": params["textDocument"]["uri"], "range": {"start": params["position"], "end": params["position"]}}])
    elif method == "textDocument/completion":
        result(identifier, {"isIncomplete": False, "items": [{"label": "naïve", "insertText": "naïve", "kind": 6}]})
    elif method == "fixture/order":
        held.append((identifier, params["value"]))
        if len(held) == 2:
            for item in reversed(held):
                result(*item)
            held.clear()
    elif method == "fixture/error":
        error(identifier, -32603, "fixture request failed")
    elif method == "fixture/hang":
        pass
    elif method == "fixture/crash":
        sys.stderr.write("fixture request crash\n")
        sys.stderr.flush()
        sys.exit(9)
    elif method == "$/cancelRequest":
        cancelled.append(params["id"])
    elif method == "fixture/cancelled":
        result(identifier, cancelled)
    elif method == "fixture/echo":
        result(identifier, params)
    elif method == "fixture/configure":
        behavior = params
        result(identifier, None)
    elif method == "fixture/notifications":
        for index in range(100):
            send({"jsonrpc": "2.0", "method": "window/logMessage", "params": {"type": 3, "message": f"{index}:" + "x" * 3000}})
        send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": "file:///synthetic.kt", "version": 1, "diagnostics": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "severity": 2, "message": "fixture diagnostic"}]}})
        result(identifier, None)
    elif method == "shutdown":
        if mode != "ignore-shutdown":
            result(identifier, None)
    elif method == "exit":
        if mode != "ignore-shutdown":
            break
    elif identifier is not None:
        error(identifier, -32601, "unknown fixture request")
