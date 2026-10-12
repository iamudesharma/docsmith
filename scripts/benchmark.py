#!/usr/bin/env python3
"""Measure CLI latency, process-tree RSS, OCR edit error, and swap snapshots.

Portable across macOS (brief target) and Linux (CI). Optional providers are only
exercised when provisioned: Apple Vision via DOC_APPLE_VISION (macOS), the local VLM
via DOC_VLM_URL/DOC_VLM_PID. Output path defaults to docs/benchmark-results.json and can
be overridden with DOC_BENCHMARK_OUTPUT, e.g. to a CI artifact directory.

DOC_BENCHMARK_STRICT=1 makes the run fail unless every case produced a result document
and the extractor process exited zero; it is a smoke gate, not an accuracy threshold.
"""
import subprocess,json,pathlib,time,os,tempfile,sys,hashlib
ROOT=pathlib.Path(__file__).resolve().parents[1];BIN=ROOT/'target/debug/doc-extractor'
OUT=pathlib.Path(os.environ.get('DOC_BENCHMARK_OUTPUT',ROOT/'docs/benchmark-results.json'))
def meminfo():
 out={}
 try:
  for line in pathlib.Path('/proc/meminfo').read_text().splitlines():
   k,_,v=line.partition(':');out[k.strip()]=v.strip()
 except OSError:pass
 return out
def swap():
 if sys.platform=='darwin':return subprocess.check_output(['sysctl','vm.swapusage'],text=True).strip()
 info=meminfo()
 if 'SwapTotal' not in info:return 'unavailable'
 total=int(info['SwapTotal'].split()[0]);free=int(info.get('SwapFree','0 kB').split()[0])
 return f"SwapTotal = {total} kB used = {total-free} kB free = {free} kB"
def hardware():
 if sys.platform=='darwin':return subprocess.check_output(['sysctl','hw.model','hw.memsize'],text=True).strip()
 info=meminfo();model='unknown'
 try:
  for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines():
   if line.startswith('model name'):model=line.split(':',1)[1].strip();break
 except OSError:pass
 return f"cpu: {model} x{os.cpu_count()}\nMemTotal: {info.get('MemTotal','?')}"
def rss(root):
 rows=[]
 for line in subprocess.check_output(['ps','-axo','pid=,ppid=,rss='],text=True).splitlines():
  try:rows.append(tuple(map(int,line.split())))
  except ValueError:pass
 ids={root}
 while True:
  old=len(ids);ids.update(pid for pid,parent,_ in rows if parent in ids)
  if len(ids)==old:break
 return sum(r*1024 for pid,parent,r in rows if pid in ids)
def edit(a,b):
 row=list(range(len(b)+1))
 for i,x in enumerate(a,1):
  nxt=[i]
  for j,y in enumerate(b,1):nxt.append(min(nxt[-1]+1,row[j]+1,row[j-1]+(x!=y)))
  row=nxt
 return row[-1]
def tesseract_languages():
 try:
  out=subprocess.run(['tesseract','--list-langs'],capture_output=True,text=True,timeout=30)
  return [l.strip() for l in out.stdout.splitlines()[1:] if l.strip()]
 except (OSError,subprocess.TimeoutExpired):return []
results=[];cases=[('quoted.csv',{}),('digital.pdf',{}),('workbook.xlsx',{}),('document.docx',{}),('diagram.drawio',{})]
for provider in ['tesseract','apple_vision']:
 if provider=='apple_vision' and 'DOC_APPLE_VISION' not in os.environ:continue
 for name in ['scan.png','noisy.png','rotated.png']:cases.append((name,{'ocr':True,'ocr_provider':provider}))
if 'hin' in tesseract_languages():cases.append(('hindi.png',{'ocr':True,'languages':'hin'}))
if 'DOC_APPLE_VISION' in os.environ and (ROOT/'fixtures/hindi.png').exists():cases.append(('hindi.png',{'ocr':True,'ocr_provider':'apple_vision','languages':'hi-IN'}))
if 'DOC_VLM_URL' in os.environ:cases.extend([('scan.png',{'vision':True}),('raster-er.png',{'vision':True,'interpret_diagrams':True})])
truth=json.loads((ROOT/'fixtures/ground-truth.json').read_text())
for name,options in cases:
 with tempfile.TemporaryDirectory() as tmp:
  tmp=pathlib.Path(tmp);(tmp/'opts.json').write_text(json.dumps(options));start=time.monotonic();before=swap();peak=0
  vlm_peak=0;vlm_before=rss(int(os.environ['DOC_VLM_PID'])) if os.environ.get('DOC_VLM_PID') and options.get('vision') else None
  p=subprocess.Popen([str(BIN),'extract',str(ROOT/'fixtures'/name),'--output',str(tmp/'out'),'--options',str(tmp/'opts.json')],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
  while p.poll() is None:
   peak=max(peak,rss(p.pid))
   if options.get('vision') and os.environ.get('DOC_VLM_PID'):vlm_peak=max(vlm_peak,rss(int(os.environ['DOC_VLM_PID'])))
   if time.monotonic()-start>140:p.kill();break
   time.sleep(.1)
  stdout,stderr=p.communicate();record={'fixture':name,'options':options,'elapsed_ms':round((time.monotonic()-start)*1000),'process_tree_peak_rss_bytes_sampled':peak,'exit':p.returncode,'swap_before':before,'swap_after':swap()}
  if (tmp/'out/result.json').exists():
   doc=json.loads((tmp/'out/result.json').read_text());record['status']=doc['status'];record['errors']=doc['errors']+[e for u in doc['units'] for e in u['errors']]
   text=' '.join(b['text'] for u in doc['units'] for b in u['blocks'] if b['provenance']['method']=='ocr');record['ocr_text']=text
   if name in truth and options.get('ocr'):
    a=' '.join(truth[name].split());b=' '.join(text.split());record['character_error_rate']=edit(a,b)/max(len(a),1)
   record['inferred_descriptions']=[b['text'] for u in doc['units'] for b in u['blocks'] if b['provenance']['method']=='inferred']
  else:record['stderr']=stderr.decode(errors='replace')[:2000]
  if options.get('vision'):
   pid=os.environ.get('DOC_VLM_PID');record['vlm_server_rss_bytes_after']=rss(int(pid)) if pid else None;record['vlm_server_rss_bytes_before']=vlm_before;record['vlm_server_peak_rss_bytes_sampled']=vlm_peak
  record['engine_binary_sha256']=hashlib.sha256(BIN.read_bytes()).hexdigest()
  if name=='raster-er.png' and (tmp/'out/result.json').exists():
   graphs=[b['diagram'] for u in doc['units'] for b in u['blocks'] if b.get('diagram')];record['inferred_graphs']=graphs
   expected=truth.get('raster_er',{});ents=[e.lower() for e in expected.get('entities',[])]
   nodes={n['id']:n.get('label','').lower() for g in graphs for n in g['nodes']};labels=list(nodes.values())
   record['expected_entities']=ents;record['entity_label_recall']=round(sum(1 for e in ents if any(e in l for l in labels))/max(len(ents),1),3)
   record['correct_endpoint_edge_count']=sum(1 for g in graphs for e in g['edges'] if all(any(x in nodes.get(e.get(k,''),'') or nodes.get(e.get(k,''),'') in x for x in ents) for k in ('source','target')))
   record['expected_edge_count']=expected.get('edge_count',1)
   if record['entity_label_recall']<1 or record['correct_endpoint_edge_count']<record['expected_edge_count']:record['accuracy_warning']='JSON may parse yet entity/edge labels differ from ground truth; all graph output remains an unverified inference.'
  results.append(record)
  if sys.platform=='darwin':
   notes=['Actual host is the recorded hw.model; the brief M1 Pro 16 GB target remains untested.']
   if any(r['options'].get('ocr_provider')=='apple_vision' for r in results):notes.append('Apple Vision failures above are real e5rt errors, not OCR quality measurements.')
   if any(r['options'].get('vision') for r in results):notes.append('Sampled VLM RSS excludes part of Metal/compressed memory; see vlm-memory-summary.txt.')
   notes.append('Synthetic fixtures are not representative document accuracy.')
  else:
   notes=['CI/shared-runner timings are smoke and regression signals, not target-hardware performance evidence.','Apple Vision and the optional VLM are not exercised on this platform in this run.','Synthetic fixtures are not representative document accuracy.']
  OUT.parent.mkdir(parents=True,exist_ok=True)
  OUT.write_text(json.dumps({'date':time.strftime('%Y-%m-%d'),'platform':sys.platform,'ci':os.environ.get('CI')=='true','hardware':hardware(),'synthetic_fixtures':True,'runs':results,'notes':notes},indent=2))
  print(name,options,record['elapsed_ms'],record.get('status'),record.get('character_error_rate'),flush=True)
if os.environ.get('DOC_BENCHMARK_STRICT'):
 missing=[r['fixture'] for r in results if 'status' not in r];nonzero=[r['fixture'] for r in results if r.get('exit',1)!=0]
 if missing or nonzero:
  print(f'STRICT FAIL missing_result={missing} nonzero_exit={nonzero}',file=sys.stderr);sys.exit(1)
print(f'{len(results)} benchmark runs written to {OUT}')
