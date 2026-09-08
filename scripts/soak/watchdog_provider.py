#!/usr/bin/env python3
"""Finite adversarial script on the existing QA wire builder; no model claims."""
import faulthandler

if __name__ == '__main__':
    faulthandler.dump_traceback_later(9)
    print('WATCHDOG_PROVIDER_STARTUP phase=imports', flush=True)

import argparse
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'qa'))
from harness import MockModel, Reply, call

REPEAT = {'command': "printf 'WATCHDOG-REPEAT\\n'; exit 1"}
STALL = {'command': '/bin/bash stall.sh', 'timeout': 0}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('phase', choices=('repeat', 'idle'))
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    payload = REPEAT if args.phase == 'repeat' else STALL
    count = 100 if args.phase == 'repeat' else 1
    script = [call('bash', **payload)] * count + [Reply(text='Adversarial script exhausted.')]
    (args.out / 'script.json').write_text(json.dumps({'phase': args.phase, 'tool': 'bash', 'arguments': payload, 'scripted_tool_responses': count}, indent=2))
    print(f'WATCHDOG_PROVIDER_STARTUP phase=bind script={args.phase}', flush=True)
    with MockModel(*script) as model:
        (args.out / 'port').write_text(str(model.port))
        faulthandler.cancel_dump_traceback_later()
        print(f'WATCHDOG_PROVIDER_LISTENING port={model.port}', flush=True)
        captured = 0
        while True:
            state = model.state()
            with (args.out / 'provider.jsonl').open('a') as log:
                for body in state['captures'][captured:]:
                    captured += 1
                    log.write(json.dumps({'ordinal': captured, 'observed_wall': time.time(), 'observed_monotonic': time.monotonic(), 'body': body}) + '\n')
            time.sleep(0.05)


if __name__ == '__main__':
    main()
