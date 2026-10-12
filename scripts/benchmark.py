#!/usr/bin/env python3
"""Measure local CLI latency, process-tree RSS, OCR edit error, and swap snapshots."""
import subprocess,json,pathlib,time,os,tempfile,sys
ROOT=pathlib.Path(__file__).resolve().parents[1];BIN=ROOT/'target/debug/doc-extractor'
def swap():return subprocess.check_output(['sysctl','vm.swapusage'],text=True).strip() if sys.platform=='darwin' else 'unavailable'
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
results=[];cases=[('quoted.csv',{}),('digital.pdf',{}),('workbook.xlsx',{}),('document.docx',{}),('diagram.drawio',{})]
for provider in ['tesseract','apple_vision']:
 if provider=='apple_vision' and 'DOC_APPLE_VISION' not in os.environ:continue
 for name in ['scan.png','noisy.png','rotated.png']:cases.append((name,{'ocr':True,'ocr_provider':provider}))
if 'TESSDATA_PREFIX' in os.environ:cases.append(('hindi.png',{'ocr':True,'languages':'hin'}))
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
  record['engine_binary_sha256']=__import__('hashlib').sha256(BIN.read_bytes()).hexdigest();
  if name=='raster-er.png' and (tmp/'out/result.json').exists():
   graphs=[b['diagram'] for u in doc['units'] for b in u['blocks'] if b.get('diagram')];record['inferred_graphs']=graphs
   expected=truth.get('raster_er',{});ents=[e.lower() for e in expected.get('entities',[])]
   nodes={n['id']:n.get('label','').lower() for g in graphs for n in g['nodes']};labels=list(nodes.values())
   record['expected_entities']=ents;record['entity_label_recall']=round(sum(1 for e in ents if any(e in l for l in labels))/max(len(ents),1),3)
   record['correct_endpoint_edge_count']=sum(1 for g in graphs for e in g['edges'] if all(any(x in nodes.get(e.get(k,''),'') or nodes.get(e.get(k,''),'') in x for x in ents) for k in ('source','target')))
   record['expected_edge_count']=expected.get('edge_count',1)
   if record['entity_label_recall']<1 or record['correct_endpoint_edge_count']<record['expected_edge_count']:record['accuracy_warning']='JSON may parse yet entity/edge labels differ from ground truth; all graph output remains an unverified inference.'
  results.append(record)
  notes=['Actual host is the recorded hw.model; the brief M1 Pro 16 GB target remains untested.','Sampled RSS excludes part of Metal/compressed memory; see vlm-memory-summary.txt.','Apple Vision failures above are real e5rt errors, not OCR quality measurements.','Synthetic fixtures are not representative document accuracy.']
  (ROOT/'docs/benchmark-results.json').write_text(json.dumps({'date':time.strftime('%Y-%m-%d'),'hardware':subprocess.check_output(['sysctl','hw.model','hw.memsize'],text=True),'synthetic_fixtures':True,'runs':results,'notes':notes},indent=2))
  print(name,options,record['elapsed_ms'],record.get('status'),record.get('character_error_rate'),flush=True)
