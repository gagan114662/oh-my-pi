#!/usr/bin/env python3
"""Controllable mock OpenAI provider for the soak workflow (#105).

Stdlib only. Serves ``POST /v1/chat/completions`` as chunked SSE with a
looping two-step script (one ``bash`` tool call, then one text reply) so that
every ``omp print`` turn is a real tool turn. Every request is appended to a
JSONL log with its timestamp, byte size, the byte length of the prefix it
shares with the previous request, and the mode that answered it.

The workflow, not omp, changes behaviour through ``POST /control``::

    {"mode": "normal"}                       # default
    {"mode": "stall", "seconds": 600}        # next request: first delta, then silence
    {"mode": "error", "status": 529}         # every request: provider error
    {"max_request_bytes": 358400}            # 400 context_length_exceeded above this
    {"retire": true}                         # model "mock" answers 404 model_not_found
    {"deltas_per_second": 20}

``GET /state`` returns counters; ``GET /log`` returns the log path.
"""

from __future__ import annotations

import faulthandler

if __name__ == "__main__":
	# Capture a blocked startup stack before the fixture's unchanged 10s gate.
	faulthandler.dump_traceback_later(9)
	print("SOAK_PROVIDER_STARTUP phase=imports", flush=True)

import argparse
import json
import os
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "qa"))
from harness import LoopbackHTTPServer, MockModel, Reply, call  # noqa: E402

SCRIPT: tuple[Reply, ...] = (
	call("bash", command="echo soak-turn; seq 1 40 | tr '\\n' ' '"),
	Reply(text="Soak turn complete: the tool ran and its forty numbers were read back. "),
)


class Control:
	def __init__(self):
		self.lock = threading.Lock()
		self.mode = "normal"
		self.stall_seconds = 600
		self.error_status = 529
		self.max_request_bytes = 0
		self.retire = False
		self.deltas_per_second = 20.0
		self.stall_pending = False

	def update(self, body: dict) -> dict:
		with self.lock:
			if "mode" in body:
				self.mode = str(body["mode"])
				self.stall_pending = self.mode == "stall"
			if "seconds" in body:
				self.stall_seconds = int(body["seconds"])
			if "status" in body:
				self.error_status = int(body["status"])
			if "max_request_bytes" in body:
				self.max_request_bytes = int(body["max_request_bytes"])
			if "retire" in body:
				self.retire = bool(body["retire"])
			if "deltas_per_second" in body:
				self.deltas_per_second = float(body["deltas_per_second"])
			return self.snapshot_locked()

	def snapshot_locked(self) -> dict:
		return {
			"mode": self.mode,
			"stall_seconds": self.stall_seconds,
			"error_status": self.error_status,
			"max_request_bytes": self.max_request_bytes,
			"retire": self.retire,
			"deltas_per_second": self.deltas_per_second,
		}


def common_prefix_len(a: bytes, b: bytes) -> int:
	n = min(len(a), len(b))
	lo, hi = 0, n
	while lo < hi:
		mid = (lo + hi + 1) // 2
		if a[:mid] == b[:mid]:
			lo = mid
		else:
			hi = mid - 1
	return lo


def main() -> None:
	parser = argparse.ArgumentParser()
	parser.add_argument("--port", type=int, default=0)
	parser.add_argument("--log", required=True, help="JSONL request log")
	parser.add_argument("--ready-file", default=None, help="written with the port once listening")
	options = parser.parse_args()

	control = Control()
	log_path = Path(options.log)
	log_lock = threading.Lock()
	state = {"served": 0, "errors": 0, "stalls": 0, "overflows": 0, "retired": 0, "previous": b""}

	def log(record: dict) -> None:
		record["ts"] = time.time()
		with log_lock:
			with log_path.open("a") as handle:
				handle.write(json.dumps(record) + "\n")

	class Handler(BaseHTTPRequestHandler):
		protocol_version = "HTTP/1.1"

		def log_message(self, *args):  # noqa: D102
			pass

		def _respond(self, status: int, content_type: str, body: bytes):
			self.send_response(status)
			self.send_header("content-type", content_type)
			self.send_header("content-length", str(len(body)))
			self.end_headers()
			self.wfile.write(body)

		def _read_body(self) -> bytes:
			if "chunked" in (self.headers.get("transfer-encoding") or "").lower():
				body = bytearray()
				while True:
					size = int(self.rfile.readline().strip().split(b";")[0] or b"0", 16)
					if size == 0:
						while self.rfile.readline().strip():
							pass
						break
					body += self.rfile.read(size)
					self.rfile.readline()
				return bytes(body)
			length = int(self.headers.get("content-length", 0))
			return self.rfile.read(length) if length else b"{}"

		def do_GET(self):
			if self.path == "/state":
				with control.lock:
					snapshot = control.snapshot_locked()
				snapshot.update({k: v for k, v in state.items() if k != "previous"})
				self._respond(200, "application/json", json.dumps(snapshot).encode())
			elif self.path == "/log":
				self._respond(200, "text/plain", str(log_path).encode())
			else:
				self._respond(404, "text/plain", b"soak-provider: unknown route")

		def do_POST(self):
			raw = self._read_body()
			if self.path == "/control":
				try:
					body = json.loads(raw or b"{}")
				except json.JSONDecodeError:
					self._respond(400, "text/plain", b"bad json")
					return
				snapshot = control.update(body)
				log({"kind": "control", **snapshot})
				self._respond(200, "application/json", json.dumps(snapshot).encode())
				return
			if not self.path.endswith("/chat/completions"):
				self._respond(404, "text/plain", b"soak-provider: unknown route")
				return
			try:
				body = json.loads(raw)
			except json.JSONDecodeError:
				body = {}
			messages_bytes = json.dumps(body.get("messages", []), sort_keys=True).encode()
			with log_lock:
				(log_path.parent / "last-request.json").write_bytes(raw or b"{}")
			with control.lock:
				snapshot = control.snapshot_locked()
				stall_now = control.stall_pending
				if stall_now:
					control.stall_pending = False
					control.mode = "normal"
			with log_lock:
				shared = common_prefix_len(state["previous"], messages_bytes)
				state["previous"] = messages_bytes
				state["served"] += 1
				ordinal = state["served"]
			record = {
				"kind": "request",
				"ordinal": ordinal,
				"bytes": len(raw),
				"messages_bytes": len(messages_bytes),
				"shared_prefix_bytes": shared,
				"model": body.get("model"),
				"mode": "stall" if stall_now else snapshot["mode"],
			}
			# --- fault modes, all decided by the workflow through /control ---
			if snapshot["retire"] and body.get("model") == "mock":
				state["retired"] += 1
				record["status"] = 404
				log(record)
				self._respond(
					404,
					"application/json",
					b'{"error":{"code":"model_not_found","message":"The model mock was retired","type":"invalid_request_error"}}',
				)
				return
			if snapshot["mode"] == "error" and not stall_now:
				state["errors"] += 1
				record["status"] = snapshot["error_status"]
				log(record)
				self._respond(
					snapshot["error_status"],
					"application/json",
					b'{"error":{"message":"soak provider overloaded","type":"overloaded_error"}}',
				)
				return
			if snapshot["max_request_bytes"] and len(raw) > snapshot["max_request_bytes"]:
				state["overflows"] += 1
				record["status"] = 400
				log(record)
				self._respond(
					400,
					"application/json",
					b'{"error":{"code":"context_length_exceeded","message":"This request exceeds the model context length","type":"invalid_request_error"}}',
				)
				return
			reply = SCRIPT[(ordinal - 1) % len(SCRIPT)]
			prompt_tokens = max(1, len(raw) // 4)
			if body.get("stream") is False:
				completion = MockModel._completion(reply, ordinal, body)
				completion["usage"] = {
					"prompt_tokens": prompt_tokens,
					"completion_tokens": 32,
					"total_tokens": prompt_tokens + 32,
				}
				record["status"] = 200
				log(record)
				self._respond(200, "application/json", json.dumps(completion).encode())
				return
			events = MockModel._sse_events(reply, ordinal, body)
			# Always report usage so compaction sees a provider receipt.
			usage = json.dumps(
				{
					"id": f"chatcmpl-soak-{ordinal}",
					"object": "chat.completion.chunk",
					"created": int(time.time()),
					"model": body.get("model", "mock"),
					"choices": [],
					"usage": {
						"prompt_tokens": prompt_tokens,
						"completion_tokens": 32,
						"total_tokens": prompt_tokens + 32,
					},
				}
			).encode()
			events = [event for event in events if event != b"[DONE]" and b'"usage"' not in event]
			events.append(usage)
			events.append(b"[DONE]")
			record["status"] = 200
			record["events"] = len(events)
			if stall_now:
				state["stalls"] += 1
				record["stall_seconds"] = snapshot["stall_seconds"]
			log(record)
			self.send_response(200)
			self.send_header("content-type", "text/event-stream")
			self.send_header("cache-control", "no-cache")
			self.send_header("transfer-encoding", "chunked")
			self.end_headers()
			delay = 1.0 / max(snapshot["deltas_per_second"], 0.1)
			try:
				for index, event in enumerate(events):
					frame = b"data: " + event + b"\n\n"
					self.wfile.write(f"{len(frame):x}\r\n".encode() + frame + b"\r\n")
					self.wfile.flush()
					if stall_now and index == 1:
						# One delta reached the client; now go silent.
						time.sleep(snapshot["stall_seconds"])
					elif index < len(events) - 2:
						time.sleep(delay)
				self.wfile.write(b"0\r\n\r\n")
				self.wfile.flush()
			except (BrokenPipeError, ConnectionResetError):
				log({"kind": "client_closed", "ordinal": ordinal})

	print("SOAK_PROVIDER_STARTUP phase=bind", flush=True)
	server = LoopbackHTTPServer(("127.0.0.1", options.port), Handler)
	port = server.server_address[1]
	if options.ready_file:
		Path(options.ready_file).write_text(str(port))
	faulthandler.cancel_dump_traceback_later()
	print(f"SOAK_PROVIDER_LISTENING port={port} log={log_path}", flush=True)
	log({"kind": "start", "port": port, "pid": os.getpid()})
	try:
		server.serve_forever()
	except KeyboardInterrupt:
		pass


if __name__ == "__main__":
	main()
