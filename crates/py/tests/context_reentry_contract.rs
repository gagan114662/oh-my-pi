//! Real Python CONTROL reader and hook callbacks during context compaction.

use omp_py::{Engine, pyo3::ffi::c_str};

#[test]
fn context_hooks_keep_reading_control_and_preserve_continuation_scope() {
	let engine = Engine::builder().init().expect("embedded Python boots");
	engine.attach(|py| py.run(c_str!(r#"
import asyncio
import json
import os
import socket
import struct

import omp
from omp._host import Host
from omp._registry import registry
from omp._context import _pending_continuation
from omp.agents import Continue
from omp.context import CompactionTier

registry.configure_manifest(
    hooks=(("agent_settled", "domain"), ("compaction", "domain")),
    extension="test/context-reentry",
)
seen = []

@omp.hook("agent_settled", name="goal")
async def goal(event, ctx):
    assert isinstance(ctx.pending_continuation, Continue)
    assert ctx.pending_continuation.prompt == "earlier proposal"
    assert omp.Context.current().pending_continuation == ctx.pending_continuation
    try:
        outcome = await omp.context.compact(tier=CompactionTier.LOCAL, focus="retain exact facts")
    except asyncio.CancelledError:
        seen.append(("parent_cancelled", ctx.cancelled()))
        raise
    assert outcome.epoch == 3
    assert ctx.pending_continuation.prompt == "earlier proposal", "nested task cannot replace parent ContextVar"
    seen.append(("parent_resumed", outcome.epoch))
    return Continue("exact next instruction", visible=False, role="system", label="goal-label", collapse_prior=True)

@omp.hook("compaction", name="inspect")
async def inspect(event, ctx):
    assert ctx.pending_continuation is None
    try:
        epoch = await omp.context.epoch()
    except asyncio.CancelledError:
        seen.append(("child_cancelled", ctx.cancelled()))
        raise
    seen.append(("nested_epoch", epoch))
    return None

registry.freeze()

settled_payload = {
    "submission_id": "submission", "reason": "stop", "committed_turns": 1,
    "last_stop": None, "pending_jobs": [], "continuations_used": 0,
    "incomplete_todos": [],
}
compaction_payload = {
    "preparation_id": "prepared", "tier": "local", "reason": "manual", "epoch": 2,
    "tokens_before": 100, "target_tokens": 20, "suggested_first_kept": "",
    "to_summarize": [], "to_retain": [], "split_turn": False,
    "previous_summary": None, "previous_preserve": None,
    "custom_instructions": "retain exact facts", "deadline": "5s",
}

async def exercise(cancel):
    host_socket, peer = socket.socketpair()
    host = Host(host_socket.fileno())
    # The fixture supplies the already frozen roster/backend. Callback execution,
    # scope installation, framing, requests and sole reader are production code.
    omp._install_control_backend(host)
    peer.setblocking(False)
    loop = asyncio.get_running_loop()
    serving = asyncio.create_task(host.serve())
    async def send(kind, correlation, body):
        frame = {"kind": kind, "body": body}
        if correlation is not None:
            frame["correlation"] = correlation
        data = json.dumps(frame).encode()
        await loop.sock_sendall(peer, struct.pack("!I", len(data)) + data)
    async def read_exact(size):
        result = b""
        while len(result) < size:
            chunk = await loop.sock_recv(peer, size - len(result))
            assert chunk, "unexpected CONTROL EOF"
            result += chunk
        return result
    async def receive():
        size, = struct.unpack("!I", await read_exact(4))
        return json.loads(await read_exact(size))
    def authority(invocation, event):
        return {
            "invocation": invocation, "extension": "test/context-reentry", "session": "session",
            "host_generation": host._host_generation, "session_generation": host._session_generation,
            "phase": "EFFECTS_AUTHORIZED", "lifecycle": "ACTIVE", "event": event,
            "principal": {"id": "test", "display": "Test"}, "trust": "trusted",
        }
    try:
        await send("Dispatch", 10, {
            "operation": "omp.hooks.dispatch", "authority": authority("parent", "agent_settled"),
            "arguments": {"event": "agent_settled", "phase": "domain", "name": "goal",
                "payload": settled_payload, "pending_continuation": {"prompt": "earlier proposal"}},
        })
        compact = await receive()
        assert compact["kind"] == "Request", compact
        assert compact["body"]["operation"] == "omp.context.compact"
        assert compact["body"]["arguments"]["focus"] == "retain exact facts"
        assert compact["body"]["authority"]["invocation"] == "parent"
        await send("Dispatch", 11, {
            "operation": "omp.hooks.dispatch", "authority": authority("child", "compaction"),
            "arguments": {"event": "compaction", "phase": "domain", "name": "inspect", "payload": compaction_payload},
        })
        epoch = await receive()
        assert epoch["kind"] == "Request", epoch
        assert epoch["body"]["operation"] == "omp.context.epoch"
        assert epoch["body"]["authority"]["invocation"] == "child"
        if cancel:
            await send("CancelDispatch", None, {"invocation": "child"})
            await send("CancelDispatch", None, {"invocation": "parent"})
            terminal = [await receive() for _ in range(4)]
            requests = {frame["correlation"] for frame in terminal if frame["kind"] == "CancelRequest"}
            assert requests == {compact["correlation"], epoch["correlation"]}, terminal
            replies = [frame for frame in terminal if frame["kind"] == "DispatchResponse"]
            assert {frame["correlation"] for frame in replies} == {10, 11}
            assert all(frame["body"]["error"]["code"] == "cancelled" for frame in replies)
            assert ("parent_cancelled", True) in seen
            assert ("child_cancelled", True) in seen
        else:
            await send("Response", epoch["correlation"], {"result": 2})
            nested = await receive()
            assert nested["kind"] == "DispatchResponse" and nested["correlation"] == 11, nested
            assert nested["body"]["result"] is None
            assert not any(row[0] == "parent_resumed" for row in seen)
            await send("Response", compact["correlation"], {"result": {
                "preparation_id": "prepared", "tiers_run": ["local"], "from_extension": None,
                "tokens_before": 100, "tokens_after": 20, "first_kept_id": "", "epoch": 3,
                "summary_bytes": 15, "warning": None,
            }})
            resumed = await receive()
            assert resumed["kind"] == "DispatchResponse" and resumed["correlation"] == 10, resumed
            assert resumed["body"]["result"] == {
                "prompt": "exact next instruction", "visible": False, "role": "system",
                "label": "goal-label", "collapse_prior": True,
            }
            assert seen == [("nested_epoch", 2), ("parent_resumed", 3)], seen
        assert not host._pending, "every deferred CONTROL waiter retired"
        assert _pending_continuation.get() is None, "callback ContextVar never escapes"
    finally:
        peer.close()
        await serving
        host_socket.close()
        omp._install_control_backend(None)

async def main():
    generations = {name: os.environ.get(name) for name in ("OMP_EXT_HOST_GENERATION", "OMP_EXT_SESSION_GENERATION")}
    os.environ["OMP_EXT_HOST_GENERATION"] = "7"
    os.environ["OMP_EXT_SESSION_GENERATION"] = "9"
    try:
        await asyncio.wait_for(exercise(False), 5)
        seen.clear()
        await asyncio.wait_for(exercise(True), 5)
    finally:
        for name, value in generations.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
asyncio.run(main())
"#), None, None)).expect("real Python CONTROL reentry contract");
}
