#!/usr/bin/env python3
"""Run every elastic-slot TLC configuration with retained, bounded evidence."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
PIN = {
    'version': '1.8.0',
    'url': 'https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar',
    'sha256': 'b658b4e504fdf0b721caf7066320f6b6fe5805f4dd2f717d0e47baba4097205e',
    'release_asset': 544648411,
}
ADR = 'docs/adr/0034/'
PAIRS = {
    'elastic': ('elastic/proof/ElasticSlots.tla', 'elastic/proof/ElasticSlots.cfg'),
    'adr': (ADR + 'ElasticSlots.tla', ADR + 'ElasticSlots.cfg'),
    'bridges': (ADR + 'ElasticSlots.tla', ADR + 'ElasticSlotsBridges.cfg'),
    'pluscal': (ADR + 'ElasticSlotsPlusCal.tla', ADR + 'ElasticSlotsPlusCal.cfg'),
    'pluscal-bridges': (ADR + 'ElasticSlotsPlusCal.tla', ADR + 'ElasticSlotsBridges.cfg'),
}
DUPLICATES = [
    ('elastic/proof/ElasticSlots.tla', ADR + 'ElasticSlots.tla'),
    ('elastic/proof/ElasticSlots.cfg', ADR + 'ElasticSlots.cfg'),
    (ADR + 'ElasticSlots.cfg', ADR + 'ElasticSlotsPlusCal.cfg'),
]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inventory(root):
    for left, right in DUPLICATES:
        if (root / left).read_bytes() != (root / right).read_bytes():
            raise ValueError('Duplicate drift: ' + left + ' differs from ' + right)
    paths = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z'], cwd=root).decode().split('\0')
    models = {p for p in paths if p.endswith('.tla')}
    model_dirs = {str(Path(p).parent) for p in models}
    configs = {p for p in paths if p.endswith('.cfg') and str(Path(p).parent) in model_dirs}
    covered_models = {model for model, _ in PAIRS.values()}
    covered_configs = {config for _, config in PAIRS.values()}
    if models != covered_models or configs != covered_configs:
        raise ValueError('Unreviewed model/config inventory: ' + repr({
            'uncovered': sorted((models | configs) - (covered_models | covered_configs)),
            'missing': sorted((covered_models | covered_configs) - (models | configs))}))
    return {p: sha(root / p) for p in sorted(models | configs)}


def acquire(path, download):
    if not path.exists():
        if not download:
            raise ValueError('Pinned TLC jar missing; pass --download or supply --jar PATH.')
        path.parent.mkdir(parents=True, exist_ok=True)
        with urllib.request.urlopen(PIN['url'], timeout=30) as source:
            data = source.read(16 * 1024 * 1024)
        if hashlib.sha256(data).hexdigest() != PIN['sha256']:
            raise ValueError('Downloaded TLC SHA-256 differs from the official release asset digest.')
        path.write_bytes(data)
    if sha(path) != PIN['sha256']:
        raise ValueError('TLC SHA-256 mismatch; refusing to execute jar.')


def execute(command, cwd, log, timeout, disk_mb):
    start = time.monotonic()
    with log.open('w') as output:
        output.write('$ ' + ' '.join(command) + '\n')
        output.flush()
        process = subprocess.Popen(command, cwd=cwd, stdout=output, stderr=subprocess.STDOUT)
        reason = None
        try:
            while process.poll() is None:
                if time.monotonic() - start > timeout:
                    reason = 'timeout'
                    break
                if sum(p.stat().st_size for p in cwd.rglob('*') if p.is_file()) > disk_mb * 1024 * 1024:
                    reason = 'disk limit'
                    break
                time.sleep(0.1)
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()
        if reason:
            output.write('\nINCOMPLETE: ' + reason + '; this is not a model-check pass.\n')
    return {'exit_code': process.returncode, 'incomplete': reason, 'seconds': round(time.monotonic() - start, 3)}


def mutate(path):
    # Deliberately drop successful retirement's semantic rows while committing
    # its frontier. ECH must catch the real state-transition error.
    old = 'history ∘ RetirementRows(c + 1, batchEnd, final, emitted[c + 1])'
    source = path.read_text()
    if source.count(old) != 1:
        raise ValueError('ECH mutation target changed; review the transition before updating proof.')
    path.write_text(source.replace(old, 'history'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--jar', type=Path, default=ROOT / 'target/tlc-tools/tla2tools-1.8.0.jar')
    parser.add_argument('--download', action='store_true', help='download official jar, enforcing pinned SHA-256')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--variant', choices=PAIRS, action='append', help='subset is reported as partial coverage')
    parser.add_argument('--timeout', type=int, default=600, help='seconds per checker invocation')
    parser.add_argument('--disk-mb', type=int, default=512)
    parser.add_argument('--heap-mb', type=int, default=1024)
    parser.add_argument('--mutate-ech', action='store_true', help='intentional failing run; never returns success')
    args = parser.parse_args()
    if min(args.timeout, args.disk_mb, args.heap_mb) <= 0:
        parser.error('timeout, disk-mb and heap-mb must be positive')
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    report = {'tool': PIN, 'runner_sha256': sha(Path(__file__)), 'variants': {}, 'status': 'failed', 'mutation': args.mutate_ech}
    try:
        report['revision'] = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
        report['sources'] = inventory(ROOT)
        acquire(args.jar, args.download)
        jar = args.jar.resolve()
        java = shutil.which('java')
        if not java:
            raise ValueError('Install Java 17+ (CI: actions/setup-java with Temurin 17).')
        report['java'] = subprocess.check_output([java, '-version'], stderr=subprocess.STDOUT, text=True)
        variants = args.variant or list(PAIRS)
        report['coverage'] = 'all' if set(variants) == set(PAIRS) else 'partial'
        for name in variants:
            model, config = PAIRS[name]
            item = {'model': model, 'config': config, 'config_text': (ROOT / config).read_text()}
            report['variants'][name] = item
            with tempfile.TemporaryDirectory(prefix='omp-tlc-') as temp:
                work = Path(temp)
                target = work / Path(model).name
                shutil.copyfile(ROOT / model, target)
                shutil.copyfile(ROOT / config, work / Path(config).name)
                if 'PlusCal' in model:
                    # Translation is generated in scratch; compare code, not timestamps/checksums.
                    before = target.read_text()
                    translated = execute([java, '-cp', str(jar), 'pcal.trans', target.name], work,
                                         args.output / (name + '-translation.txt'), args.timeout, args.disk_mb)
                    item['translation'] = translated
                    after = target.read_text()
                    body = lambda text: text.split('\\* BEGIN TRANSLATION', 1)[1].split('\n', 1)[1].split('\\* END TRANSLATION', 1)[0].strip()
                    if translated['exit_code'] != 0 or translated['incomplete'] or body(before) != body(after):
                        item['status'] = 'translation drift or failure'
                        shutil.copyfile(target, args.output / (name + '-generated.tla'))
                        continue
                if args.mutate_ech:
                    if 'PlusCal' in model:
                        item['status'] = 'mutation unsupported for PlusCal; use --variant bridges'
                        continue
                    mutate(target)
                item['executed_model_sha256'] = sha(target)
                shutil.copyfile(target, args.output / (name + '-checked.tla'))
                shutil.copyfile(work / Path(config).name, args.output / (name + '-checked.cfg'))
                # Retain a failed/in-progress report even if the CI step is interrupted.
                (args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
                log = args.output / (name + '-tlc-output.txt')
                result = execute([java, '-XX:+UseParallelGC', '-Xmx' + str(args.heap_mb) + 'm',
                                  '-cp', str(jar), 'tlc2.TLC', '-workers', '1', '-seed', '1',
                                  '-config', Path(config).name, target.name], work, log, args.timeout, args.disk_mb)
                item.update(result)
                text = log.read_text()
                item['log_sha256'] = sha(log)
                item['counterexample'] = 'Invariant ExactCommittedHistory is violated' in text and 'State 2:' in text
                item['status'] = 'passed' if result['exit_code'] == 0 and not result['incomplete'] and 'Model checking completed. No error has been found.' in text else 'failed or incomplete'
                (args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
        if all(item.get('status') == 'passed' for item in report['variants'].values()) and not args.mutate_ech:
            report['status'] = 'passed' if report['coverage'] == 'all' else 'partial'
    except (OSError, ValueError, subprocess.SubprocessError, IndexError) as error:
        report['error'] = str(error)
    (args.output / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    lines = ['# Elastic slots TLC evidence', '', 'Status: **' + report['status'] + '**', '',
             'Jar SHA-256: `' + PIN['sha256'] + '`', '', '| Variant | Status | Log |', '|---|---|---|']
    for name, item in report['variants'].items():
        lines.append(f"| {name} | {item.get('status', 'failed')} | {name}-tlc-output.txt |")
    for name, item in report['variants'].items():
        lines.extend(['', 'Configuration for ' + name + ':', '```', item['config_text'], '```', ''])
        if item.get('counterexample'):
            log_text = (args.output / (name + '-tlc-output.txt')).read_text()
            trace = log_text[log_text.index('Error: Invariant'):][:20000]
            lines.extend(['ECH counterexample generated by TLC (full trace in artifact):', '```text', trace, '```', ''])
    if 'error' in report:
        lines.extend(['', report['error']])
    lines.extend(['', 'Timeouts, parse failures, and partial coverage are not full proof. TLC checks the model, not the Rust implementation.'])
    summary = '\n'.join(lines) + '\n'
    (args.output / 'summary.md').write_text(summary)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as output:
            output.write(summary)
    print(summary)
    return 0 if report['status'] in ['passed', 'partial'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
