# Approved one-shot scope lifetime and remaining race

The compiled `FilePolicy` retains the original `ApprovedPathScope`, including
its Unix inode handle. `ExecSandboxAttempt` retains that policy through its
sandbox owner. Dropping the approval caller's copy cannot release the inode
while the compiled policy or an attempt remains alive.

In-process path checks revalidate the approved identity for effects under its
scope. The actual file-open path invokes those checks before opening. External
shell command composition invokes `SpawnWrapper::validate`; detached shell and
Python worker command constructors also revalidate and propagate failure.
Unchanged approvals remain usable; a replaced directory cannot inherit the
old approval merely by occupying the same pathname.

This does not close issue #68's complete confinement requirement. Validation
and the later descriptor-relative file open are separate operations. External
command construction, process launch, and backend path-based rule installation
are also separate operations. An adversary can still replace a path between a
successful check and later use. These checks reject an already changed scope;
they are not atomic kernel confinement or a descriptor-bound backend grant.

The regression `compiled_attempt_retains_and_rechecks_approved_scope` uses the
production policy compiler and attempt file-open path, drops the caller scope,
replaces the directory, and checks rejection of file access and command
construction. Hosted Linux/macOS execution remains required; this document is
not evidence of an executed test.
