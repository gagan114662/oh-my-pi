#!/usr/bin/env python3
"""Inventory native Tool implementations against the advertised family list.

This source inventory complements the compiled envd registry/lowering test. It
is not proof that an LSP server or browser process successfully executes work.
"""
import argparse
import json
import re
from pathlib import Path

# These are module-to-family naming relationships, not a second tool roster.
ALIASES = {
    "bash": "shell", "yield": "yield_tool", "recall": "memory",
    "reflect": "memory", "retain": "memory", "rewind": "checkpoint",
}
HOST_IMPLEMENTATIONS = {
    "image_gen": "crates/envd/src/media_devices.rs",
    "tts": "crates/envd/src/media_devices.rs",
    "report_issue": "crates/envd/src/report_issue.rs",
}

def inspect(root, identity_source=None):
    source = identity_source if identity_source is not None else (root / 'crates/tools/src/lib.rs').read_text()
    block = source.split('const BUILTIN_TOOL_IDENTITIES:', 1)[1].split('];', 1)[0]
    identities = re.findall(r'BuiltinToolIdentity\s*\{\s*name:\s*"([^"]+)",\s*hidden:\s*(true|false)\s*\}', block)
    if not identities:
        raise ValueError('No builtin identities parsed')
    errors, rows, covered = [], [], set()
    names = [name for name, hidden in identities]
    if len(names) != len(set(names)):
        errors.append('Duplicate builtin family identity')
    tool_root = root / 'crates/tools/src'
    implemented = set()
    for path in tool_root.rglob('*.rs'):
        if re.search(r'\bTool\s+for\b', path.read_text()):
            implemented.add(path.relative_to(tool_root).parts[0].removesuffix('.rs'))
    for name, hidden in identities:
        module = ALIASES.get(name, name)
        if name in HOST_IMPLEMENTATIONS:
            implementation = HOST_IMPLEMENTATIONS[name]
            if not (root / implementation).exists():
                errors.append(f'{name}: host implementation missing')
        else:
            paths = [tool_root / f'{module}.rs', tool_root / module / 'mod.rs']
            path = next((path for path in paths if path.exists()), None)
            implementation = str(path.relative_to(root)) if path else f'MISSING:{module}'
            if module not in implemented:
                errors.append(f'{name}: no native Tool implementation in {module}')
            covered.add(module)
        rows.append({'family': name, 'implementation': implementation,
                     'listing': 'hidden/explicit selection' if hidden == 'true' else 'builtin family list',
                     'runtime_surface': 'resolved by production registry policy and capability settings'})
    required = set(HOST_IMPLEMENTATIONS)
    for module in implemented:
        required.update(name for name, alias in ALIASES.items() if alias == module)
        if module not in ('shell', 'yield_tool', 'memory'):
            required.add(module)
    for name in sorted(required - set(names)):
        errors.append(f'Implemented native family {name} is absent from the identity catalog')
    for module in sorted(implemented - covered):
        errors.append(f'Implemented tool module {module} has no builtin family identity')
    return {'families': rows, 'implemented_modules': sorted(implemented), 'errors': errors,
            'scope': 'Source inventory; compiled envd policy test separately checks actual slot/device lowering.'}

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--output', type=Path, default=Path('target/tool-roster'))
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    root = args.root.resolve()
    report = inspect(root)
    if args.self_test:
        source = (root / 'crates/tools/src/lib.rs').read_text()
        # Prove the actual missing-family defect fails, for every unique module.
        for name in (row['family'] for row in report['families']):
            altered = re.sub(r'\s*BuiltinToolIdentity\s*\{\s*name:\s*"' + name + r'",\s*hidden:\s*(?:true|false)\s*\},', '', source)
            if not inspect(root, altered)['errors']:
                raise SystemExit(f'Negative check failed to reject missing {name}')
        print(f"Negative inventory checks rejected all {len(report['families'])} missing native families")
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / 'coverage.json').write_text(json.dumps(report, indent=2) + '\n')
    lines = ['# Native tool source coverage', '', report['scope'], '', '| Family | Implementation | Listing |', '| --- | --- | --- |']
    for row in report['families']:
        lines.append(f"| {row['family']} | {row['implementation']} | {row['listing']} |")
    lines.extend(['', 'Conditional availability is not absence from the identity catalog. Default long-tail tools use dyn; tool-only policy exposes slots. Hidden families require explicit selection.', '', *report['errors']])
    (args.output / 'coverage.md').write_text('\n'.join(lines) + '\n')
    print(f"{len(report['families'])} families; {len(report['implemented_modules'])} native implementation modules")
    if report['errors']:
        raise SystemExit('\n'.join(report['errors']))

if __name__ == '__main__':
    main()
