#!/usr/bin/env python3
"""Export bounded original-model action traces for public Slots API replay."""
import argparse, hashlib, json, shutil, subprocess, tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODEL = ROOT / 'docs/adr/0034/ElasticSlots.tla'
JAR_SHA = 'b658b4e504fdf0b721caf7066320f6b6fe5805f4dd2f717d0e47baba4097205e'
# Schedules select ORIGINAL model actions. Expected states come only from TLC.
SCENARIOS = {
 'mutable_fifo': {'height': 2, 'blocks': 2, 'actions': [
  'Create("Mutable")', 'Admit(1)', 'Update(1, <<"row-a">>)',
  'Create("Mutable")', 'Admit(2)', 'Update(2, <<"row-b">>)',
  'FinalizeActive(2, <<"row-b">>)', 'FinalizeActive(1, <<"row-a">>)',
  'BeginFlush', 'RetireSuccess(2)']},
 'append_prefix': {'height': 1, 'blocks': 1, 'actions': [
  'Create("AppendOnly")', 'Admit(1)', 'Update(1, <<"row-a", "row-b">>)',
  'AppendStable', 'FinalizeActive(1, <<"row-a", "row-b">>)',
  'BeginFlush', 'RetireSuccess(1)']},
}

def generate(jar, output):
 if hashlib.sha256(jar.read_bytes()).hexdigest() != JAR_SHA: raise ValueError('Unpinned TLC jar')
 output.mkdir(parents=True, exist_ok=True)
 cases=[]
 for name, scenario in SCENARIOS.items():
  with tempfile.TemporaryDirectory(prefix='omp-elastic-trace-') as temporary:
   work=Path(temporary)
   shutil.copyfile(MODEL,work/'ElasticSlots.tla')
   actions=scenario['actions']
   next_steps='\n    \\/ '.join(f'(pc = {index} /\\ {action} /\\ pc\' = {index+1})' for index,action in enumerate(actions))
   driver=f'''---- MODULE ReplaySchedule ----
EXTENDS ElasticSlots
VARIABLE pc
TraceInit == Init /\\ pc = 0
TraceNext == {next_steps}
TraceNotFinished == pc < {len(actions)}
====
'''
   (work/'ReplaySchedule.tla').write_text(driver)
   # Keep every original safety invariant. The only expected violation is the
   # trace-completion sentinel, used to ask TLC to export the completed path.
   config=(ROOT/'docs/adr/0034/ElasticSlots.cfg').read_text()
   config=config.replace('SPECIFICATION Spec','INIT TraceInit\nNEXT TraceNext')
   config=config.replace('N = 2',f'N = {scenario["blocks"]}').replace('H = 1',f'H = {scenario["height"]}')
   config=config.replace('SnapshotValues <- SmallModelSnapshots','SnapshotValues <- ModelSnapshots')
   config=config.split('PROPERTIES',1)[0]+'    TraceNotFinished\n\nCHECK_DEADLOCK FALSE\n'
   (work/'ReplaySchedule.cfg').write_text(config)
   result=subprocess.run(['java','-cp',str(jar.resolve()),'tlc2.TLC','-workers','1','-noGenerateSpecTE','-config','ReplaySchedule.cfg','-dumpTrace','json','trace.json','ReplaySchedule'],cwd=work,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=60)
   (output/f'{name}.log').write_text(result.stdout)
   if result.returncode == 0 or 'Invariant TraceNotFinished is violated' not in result.stdout:
    raise RuntimeError(f'{name}: did not reach requested model trace; see {output/name}.log')
   trace=json.loads((work/'trace.json').read_text())
   (output/f'{name}-raw.json').write_text(json.dumps(trace,indent=2)+'\n')
   cases.append({'name':name,**scenario,'trace':trace})
 result={'model':'docs/adr/0034/ElasticSlots.tla','model_sha256':hashlib.sha256(MODEL.read_bytes()).hexdigest(),'tlc_sha256':JAR_SHA,'scope':'Wide unwrapped rows; admitted blocks; mutable FIFO and append-only prefix; no resize/failure refinement claim','cases':[]}
 for case in cases:
  edges=case['trace']['counterexample']['action']
  states=[edges[0][0][1]]+[edge[2][1] for edge in edges]
  if [state['pc'] for state in states] != list(range(len(case['actions'])+1)): raise ValueError('Incomplete/nonlinear TLC trace')
  steps=[]
  for index,state in enumerate(states):
   action='Init' if index==0 else case['actions'][index-1]
   steps.append({'action':action,'phase':state['phase'],'mode':state['mode'],'want':state['want'],'emitted':state['emitted'],'frontier':state['c'],'history':state['history']})
  result['cases'].append({'name':case['name'],'height':case['height'],'steps':steps})
 (output/'traces.json').write_text(json.dumps(result,indent=2)+'\n')
 return result

if __name__=='__main__':
 parser=argparse.ArgumentParser();parser.add_argument('--jar',type=Path,required=True);parser.add_argument('--output',type=Path,default=ROOT/'target/elastic-refinement');parser.add_argument('--check',action='store_true');args=parser.parse_args()
 cases=generate(args.jar,args.output)
 if args.check and cases != json.loads((ROOT/'crates/tui/tests/fixtures/elastic-model-traces.json').read_text()):
  raise SystemExit('Generated TLC traces differ from checked-in replay expectations')
 print(f"Exported {len(cases['cases'])} original-model traces to {args.output/'traces.json'}")
