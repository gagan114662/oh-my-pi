//! Regression proof for canonical tool identities crossing CONTROL startup.

use omp_py::{Engine, pyo3::ffi::c_str};

#[test]
fn canonical_control_identities_and_legacy_availability_are_preserved() {
	let engine = Engine::builder().init().expect("embedded Python boots");
	engine.attach(|py| {
        py.run(c_str!(r#"
from types import SimpleNamespace
import omp
import omp._host as host_module
import omp._registry as registry_module

class BootstrapReached(Exception):
    pass

def stop_before_import():
    raise BootstrapReached

def accept_tier(row):
    host = SimpleNamespace(
        _backend_installed=False,
        _host_generation=1,
        _session_generation=2,
        bootstrap_registry=stop_before_import,
    )
    frame = SimpleNamespace(kind="AuthoritySnapshot", body={
        "host_generation": 1,
        "session_generation": 2,
        "agent_depth": 0,
        "tiers": [row],
    })
    try:
        host_module.Host._accept(host, frame)
    except BootstrapReached:
        return host._tier_snapshot
    raise AssertionError("authority did not reach registry bootstrap")

assert accept_tier({"kind": "device", "name": "echo", "family": "", "rev": "1", "tier": "trusted"}) == {
    ("device", "echo", "", "1"): "trusted"
}
assert accept_tier({"kind": "device", "name": "echo", "family": "r", "rev": "1", "tier": "trusted"}) == {
    ("device", "echo", "r", "1"): "trusted"
}
for field, value in (("name", ""), ("rev", ""), ("family", None), ("family", 1)):
    row = {"kind": "device", "name": "echo", "family": "", "rev": "1", "tier": "trusted"}
    row[field] = value
    try:
        accept_tier(row)
    except host_module.HostDisconnected:
        pass
    else:
        raise AssertionError(f"malformed identity accepted: {row!r}")
for row in (
    {"kind": "core", "name": "", "rev": "1", "tier": "trusted"},
    {"kind": "mcp", "server": "", "tool": "echo", "tier": "trusted"},
):
    try:
        accept_tier(row)
    except host_module.HostDisconnected:
        pass
    else:
        raise AssertionError(f"malformed identity accepted: {row!r}")

omp.packages._install_snapshot([{
    "name": "legacy-contract", "version": "1.0.0", "extension_id": "legacy-contract",
    "root": "/legacy-contract", "files": (),
}], own="legacy-contract")
registry = registry_module.DeclarationRegistry()
registry.configure_manifest(extension="legacy-contract", declarations=[{
    "id": "echo@r.1", "kind": "soft", "module": "__main__",
    "key": "echo@r.1", "trigger": "lazy", "api": 1, "failure": "fault",
}])
def echo(params):
    return params
registry.register_legacy_worker_tool({
    "name": "echo", "rev": "r.1", "handler": echo,
    "description": "echo", "schema": {"type": "object"},
})
registry.freeze()
original = registry_module.registry
registry_module.registry = registry
try:
    publication = registry_module.project_control_registry()
finally:
    registry_module.registry = original
assert publication["declaration_keys"] == [{"kind": "soft", "key": "echo@r.1"}]
assert [(t["name"], t["family"], t["rev"], t["kind"]) for t in publication["tools"]] == [("echo", "r", 1, "soft")]
assert publication["availability"] == [{
    "name": "echo", "family": "r", "rev": 1, "mounted": True, "reason": None,
}]
"#), None, None).expect("canonical CONTROL identities and availability");
    });
}
