#!/usr/bin/env python3
"""Render a benchmark-results.json as Markdown for a CI job summary or terminal."""
import json,sys,pathlib
def render(path):
 d=json.loads(pathlib.Path(path).read_text())
 lines=[f"## Benchmark results — {d.get('date','?')} ({d.get('platform','?')}{', CI' if d.get('ci') else ''})","","```",d.get('hardware','').strip(),"```",""]
 lines+=["| Fixture | Options | ms | Status | CER | Peak RSS MB | Exit |","|---|---|---|---|---|---|---|"]
 for r in d.get('runs',[]):
  options=json.dumps(r.get('options',{}),separators=(',',':')).replace('|','\\|')
  cer=r.get('character_error_rate');cer='' if cer is None else f"{cer:.3f}"
  rss=round(r.get('process_tree_peak_rss_bytes_sampled',0)/1e6,1)
  lines.append(f"| {r.get('fixture')} | `{options}` | {r.get('elapsed_ms')} | {r.get('status','failed')} | {cer} | {rss} | {r.get('exit')} |")
 lines+=[""]
 for w in [r for r in d.get('runs',[]) if r.get('accuracy_warning')]:lines.append(f"- Warning: `{w['fixture']}`: {w['accuracy_warning']}")
 for n in d.get('notes',[]):lines.append(f"- Note: {n}")
 return "\n".join(lines)+"\n"
if __name__=='__main__':
 if len(sys.argv)!=2:print(f"usage: {sys.argv[0]} benchmark-results.json",file=sys.stderr);sys.exit(2)
 print(render(sys.argv[1]),end='')
