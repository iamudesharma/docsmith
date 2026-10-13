#!/usr/bin/env python3
"""Real HTTP/SQLite/isolated-worker acceptance checks, no production Python service."""
import pathlib,tempfile,subprocess,json,time,urllib.request,urllib.error,os,signal,socket,sys,http.server,threading,hmac,hashlib
ROOT=pathlib.Path(__file__).resolve().parents[1];BIN=ROOT/'target/debug/doc-extractor'
results=[]
with tempfile.TemporaryDirectory(prefix='doc-extractor-e2e-') as tmp:
 tmp=pathlib.Path(tmp)
 with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
 base=f'http://127.0.0.1:{port}'
 config={'data_dir':str(tmp/'data'),'bind':f'127.0.0.1:{port}','tokens':{'a-token':'tenant-a','b-token':'tenant-b'},'webhook_secret':'e2e-secret','webhook_allow_private':True,'sink_command':'cp {result} sink-{state}.out','limits':{'workers':1,'timeout_seconds':4,'queued_jobs':3,'upload_bytes':131072}}
 (tmp/'config.json').write_text(json.dumps(config))
 # Slow OCR provider makes cancellation/timeout tests deterministic and tests whole process groups.
 (tmp/'bin').mkdir();(tmp/'bin/tesseract').write_text('#!/bin/sh\nif [ "$1" = --list-langs ]; then printf "List\\neng\\n"; exit 0; fi\nif [ "$1" = --version ]; then echo "tesseract mock"; exit 0; fi\nsleep 20\n');(tmp/'bin/tesseract').chmod(0o755)
 env=dict(os.environ,PATH=str(tmp/'bin')+':'+os.environ['PATH'])
 # Local webhook receiver proves signed delivery end to end.
 received=[]
 class _Hook(http.server.BaseHTTPRequestHandler):
  def do_POST(self):
   body=self.rfile.read(int(self.headers.get('content-length',0)))
   received.append((self.headers,body));self.send_response(200);self.end_headers();self.wfile.write(b'ok')
  def log_message(self,*_a):pass
 hook=http.server.HTTPServer(('127.0.0.1',0),_Hook);hook_port=hook.server_address[1]
 threading.Thread(target=hook.serve_forever,daemon=True).start()
 def start():
  p=subprocess.Popen([str(BIN),'serve','--config',str(tmp/'config.json')],env=env,stdout=subprocess.DEVNULL,stderr=open(tmp/'server.log','a'))
  for _ in range(100):
   try:
    if request('/health/live',token=None)[0]==200:return p
   except OSError:pass
   if p.poll() is not None:raise AssertionError((tmp/'server.log').read_text())
   time.sleep(.05)
  raise AssertionError('server did not start')
 def request(path,method='GET',data=None,token='a-token',headers=None,root=None):
  h=headers or {}
  if token:h['Authorization']='Bearer '+token
  req=urllib.request.Request((root or base)+path,data=data,method=method,headers=h)
  try:
   with urllib.request.urlopen(req,timeout=10) as r:return r.status,r.read()
  except urllib.error.HTTPError as e:return e.code,e.read()
 def upload(name,options=None,token='a-token',content=None,root=None,raw_options=None,callback=None):
  boundary='fixture-boundary'
  b=f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: application/octet-stream\r\n\r\n'.encode()+((ROOT/'fixtures'/name).read_bytes() if content is None else content)+b'\r\n'
  if options is not None:b+=f'--{boundary}\r\nContent-Disposition: form-data; name="options"\r\n\r\n'.encode()+json.dumps(options).encode()+b'\r\n'
  elif raw_options is not None:b+=f'--{boundary}\r\nContent-Disposition: form-data; name="options"\r\n\r\n'.encode()+raw_options.encode()+b'\r\n'
  if callback is not None:b+=f'--{boundary}\r\nContent-Disposition: form-data; name="callback_url"\r\n\r\n'.encode()+callback.encode()+b'\r\n'
  b+=f'--{boundary}--\r\n'.encode()
  code,body=request('/v1/extractions','POST',b,token,{'Content-Type':'multipart/form-data; boundary='+boundary},root=root);return code,json.loads(body)
 def status(id,root=None):return json.loads(request('/v1/extractions/'+id,root=root)[1])
 def wait(id,target=None,root=None):
  for _ in range(160):
   v=status(id,root=root)
   if (target and v['state']==target) or (not target and v['state'] in ('completed','failed','cancelled')):return v
   time.sleep(.05)
  raise AssertionError(v)
 def wait_gone(id,root=None,seconds=10):
  deadline=time.time()+seconds
  while time.time()<deadline:
   if request('/v1/extractions/'+id,root=root)[0]==404:return True
   time.sleep(.25)
  return False
 def check(name,condition):assert condition,name;results.append(name)
 p=None;p2=None;p3=None
 try:
  p=start();check('health readiness',request('/health/ready',token=None)[0]==200)
  check('unauthorized upload rejected',upload('quoted.csv',token=None)[0]==401)
  code,j=upload('quoted.csv');check('multipart 202',code==202);id=j['id'];v=wait(id);check('durable complete '+str(v),v['state']=='completed')
  sink_path=tmp/'data/jobs'/id/'sink-completed.out'
  deadline=time.time()+8
  while not sink_path.exists() and time.time()<deadline:time.sleep(.2)
  check('sink command runs on completion',sink_path.exists() and sink_path.stat().st_size>0)
  code,body=request(f'/v1/extractions/{id}/result');d=json.loads(body);check('canonical cells',d['units'][0]['blocks'][0]['table']['cells'][5]['raw']=='0012')
  check('unit endpoint',request(f'/v1/extractions/{id}/units/u1')[0]==200)
  check('derived markdown',b'0012' in request(f'/v1/extractions/{id}/result?format=markdown')[1])
  check('plain text includes cells',b'0012' in request(f'/v1/extractions/{id}/result?format=text')[1])
  _,j=upload('quoted.csv',{'result_format':'text'});txtjob=j['id'];wait(txtjob)
  txtbody=request(f'/v1/extractions/{txtjob}/result')[1]
  check('result_format text is the default export',b'\t' in txtbody and b'0012' in txtbody and not txtbody.startswith(b'{'))
  check('explicit format overrides the job default',request(f'/v1/extractions/{txtjob}/result?format=json')[1].startswith(b'{'))
  check('invalid result_format rejected',upload('quoted.csv',{'result_format':'pdf'})[0]==400)
  _,j=upload('quoted.csv',{'compact_tables':True});compact=j['id'];wait(compact)
  raw=request(f'/v1/extractions/{compact}/result')[1]
  check('compact tables omit redundant cell fields',b'"value"' not in raw and b'"cached_value"' not in raw and b'"raw"' in raw)
  check('tenant isolation',request(f'/v1/extractions/{id}',token='b-token')[0]==404)
  check('tenant-specific cache hit',upload('quoted.csv')[1]['id']==id)
  other=upload('quoted.csv',token='b-token')[1]['id'];check('cache does not cross tenants',other!=id)
  check('invalid options rejected',upload('quoted.csv',{'unknown':True})[0]==400)
  check('empty or blank options part uses defaults',upload('quoted.csv',raw_options='')[0]==202 and upload('quoted.csv',raw_options=' ')[0]==202)
  check('streamed upload limit returns 413',upload('quoted.csv',content=b'x'*140000)[0]==413)
  check('body limit returns 413',upload('quoted.csv',content=b'x'*180000)[0]==413)
  _,j=upload('document.docx');assetjob=j['id'];wait(assetjob);d=json.loads(request(f'/v1/extractions/{assetjob}/result')[1]);asset=d['assets'][0]['id'];check('asset endpoint',request(f'/v1/extractions/{assetjob}/assets/{asset}')[0]==200)
  check('traversal rejected',request(f'/v1/extractions/{assetjob}/assets/a..')[0]==404)
  _,j=upload('scan.png',{'ocr':True});slow=j['id'];wait(slow,'running')
  _,j=upload('flexible.tsv');queued=j['id'];check('queued cancellation',json.loads(request(f'/v1/extractions/{queued}/cancel','POST',b'')[1])['state']=='cancelled')
  check('running cancellation',json.loads(request(f'/v1/extractions/{slow}/cancel','POST',b'')[1])['state']=='cancelled')
  time.sleep(.2);check('cancel remains cancelled',status(slow)['state']=='cancelled')
  check('double cancellation is idempotent',json.loads(request(f'/v1/extractions/{slow}/cancel','POST',b'')[1])['state']=='cancelled')
  # Cancellation must reap the whole process group (worker + stub tesseract + sleep) and leave
  # no partially written temp files behind.
  time.sleep(.5)
  descendants=subprocess.run(['pgrep','-f','sleep 20'],capture_output=True,text=True).stdout.strip()
  check('cancellation reaps OCR descendants',descendants=='')
  jobdir=tmp/'data/jobs'/slow
  tempfiles=[f.name for f in jobdir.iterdir() if '.tmp-' in f.name] if jobdir.exists() else []
  deadline=time.time()+3
  while tempfiles and time.time()<deadline:
   time.sleep(.2)
   tempfiles=[f.name for f in jobdir.iterdir() if '.tmp-' in f.name] if jobdir.exists() else []
  check('cancellation leaves no temp files',tempfiles==[])
  # DELETE of a running job must stop the isolated worker and remove retained data.
  _,j=upload('scan.png',{'ocr':True});deleting=j['id'];wait(deleting,'running')
  code,_=request(f'/v1/extractions/{deleting}','DELETE')
  if code==409:time.sleep(.4);code,_=request(f'/v1/extractions/{deleting}','DELETE')
  check('delete while running stops worker',code==204 and request(f'/v1/extractions/{deleting}')[0]==404)
  _,j=upload('scan.png',{'ocr':True,'languages':'eng'});timeout=j['id'];check('enforced process timeout',wait(timeout)['state']=='failed')
  # Crash/restart a running worker; recovery must rerun from safe persisted input/checkpoints.
  _,j=upload('noisy.png',{'ocr':True});recover=j['id'];wait(recover,'running');time.sleep(.2)
  p.kill();p.wait();p=start();v=wait(recover);check('crash restart recovers job',v['state']=='failed' and v['error']['code']=='limit_exceeded')
  check('delete removes job and data',request(f'/v1/extractions/{id}','DELETE')[0]==204 and request(f'/v1/extractions/{id}')[0]==404 and not (tmp/'data/jobs'/id).exists())
  # Landing page, listing, stats, JSONL, unit slicing, SSE, archive and signed webhooks.
  home_code,home=request('/')
  check('landing page',home_code==200 and b'/docs' in home and b'/v1/stats' in home)
  _,j=upload('quoted.csv',{'result_format':'jsonl'});jsonljob=j['id'];wait(jsonljob)
  nd=request(f'/v1/extractions/{jsonljob}/result')[1]
  check('result_format jsonl streams records',nd.count(b'\n')>=3 and b'"cells"' in nd and b'"row":' in nd)
  listing=json.loads(request('/v1/extractions?limit=5')[1])
  check('job listing',listing['total']>=1 and any(x['id']==jsonljob for x in listing['jobs']))
  check('job listing state filter',all(x['state']=='completed' for x in json.loads(request('/v1/extractions?state=completed&limit=200')[1])['jobs']))
  st=json.loads(request('/v1/stats')[1])
  check('stats endpoint',st['jobs'].get('completed',0)>=1 and st['workers']['slots']==1 and st['disk_bytes']>0)
  sliced=json.loads(request(f'/v1/extractions/{jsonljob}/units/u1?from=1&count=1')[1])
  check('unit row slicing',sliced['slice']['rows']==1 and sliced['unit']['blocks'][0]['table']['rows']==1)
  events=request(f'/v1/extractions/{jsonljob}/events')[1]
  check('sse job events',b'data:' in events and b'"completed"' in events)
  arch=request(f'/v1/extractions/{jsonljob}/archive')[1]
  check('archive zip',arch[:2]==b'PK')
  view_code,view_page=request(f'/view/{jsonljob}')
  check('viewer page',view_code==200 and b'doc-extractor viewer' in view_page)
  metrics=request('/metrics')[1]
  check('prometheus metrics',b'doc_jobs{' in metrics and b'doc_workers_slots' in metrics)
  _,j=upload('flexible.tsv',callback=f'http://127.0.0.1:{hook_port}/hook');hookjob=j['id'];wait(hookjob)
  deadline=time.time()+10
  while not received and time.time()<deadline:time.sleep(.1)
  check('webhook delivery on completion',bool(received))
  headers,body=received[0];payload=json.loads(body)
  ts=headers.get('x-docsmith-timestamp')
  expected='sha256='+hmac.new(b'e2e-secret',f'{ts}.{body.decode()}'.encode(),hashlib.sha256).hexdigest()
  check('webhook signature and payload',headers.get('x-docsmith-event')=='job.completed' and payload['job']['id']==hookjob and headers.get('x-docsmith-signature')==expected)
  spec=json.loads(request('/openapi.json')[1]);check('openapi routes',len(spec['paths'])==12)
  docs_code,docs_page=request('/docs');docs_spec=json.loads(request('/docs/openapi.json')[1])
  check('swagger docs ui serves the same contract',docs_code==200 and b'Swagger UI' in docs_page and docs_spec['paths'].keys()==spec['paths'].keys())
  # Second instance proves a two-worker pool and the retention sweep (no quota).
  with socket.socket() as s2:s2.bind(('127.0.0.1',0));port2=s2.getsockname()[1]
  base2=f'http://127.0.0.1:{port2}'
  (tmp/'config2.json').write_text(json.dumps({'data_dir':str(tmp/'data2'),'bind':f'127.0.0.1:{port2}','limits':{'workers':2,'ocr_threads':1,'timeout_seconds':30,'queued_jobs':10,'upload_bytes':131072,'retention_seconds':2}}))
  p2=subprocess.Popen([str(BIN),'serve','--config',str(tmp/'config2.json')],env=env,stdout=subprocess.DEVNULL,stderr=open(tmp/'server2.log','a'))
  for _ in range(100):
   try:
    if request('/health/live',token=None,root=base2)[0]==200:break
   except OSError:pass
   time.sleep(.05)
  _,j=upload('scan.png',{'ocr':True},token=None,root=base2);busy=j['id'];wait(busy,'running',root=base2)
  _,j=upload('quoted.csv',token=None,root=base2);fast=j['id'];v=wait(fast,root=base2)
  check('second worker completes a job while one is busy',v['state']=='completed' and status(busy,root=base2)['state'] in ('running','queued'))
  check('private callback refused without opt-in',upload('quoted.csv',callback='http://127.0.0.1:9/hook',token=None,root=base2)[0]==400)
  # Machine-wide OCR budget: config2 sets ocr_threads=1, so even with two workers busy on two
  # different OCR jobs, at most one stub Tesseract may run across both worker processes.
  _,j=upload('scan.png',{'ocr':True},token=None,root=base2);second_ocr=j['id']
  for _ in range(40):
   if status(second_ocr,root=base2)['state']=='running':break
   time.sleep(.05)
  max_concurrent=0
  for _ in range(10):
   running=len(subprocess.run(['pgrep','-f','sleep 20'],capture_output=True,text=True).stdout.strip().split())
   max_concurrent=max(max_concurrent,running)
   time.sleep(.2)
  check('machine-wide OCR budget shared across jobs',max_concurrent==1)
  json.loads(request(f'/v1/extractions/{second_ocr}/cancel','POST',b'',token=None,root=base2)[1])
  json.loads(request(f'/v1/extractions/{busy}/cancel','POST',b'',token=None,root=base2)[1])
  check('retention sweep removes terminal jobs',wait_gone(fast,root=base2))
  p2.send_signal(signal.SIGINT);p2.wait(timeout=10);p2=None
  # Third instance proves the disk quota deterministically: one completed job already
  # exceeds the limit, so a cache hit is still served while new storage is rejected.
  with socket.socket() as s3:s3.bind(('127.0.0.1',0));port3=s3.getsockname()[1]
  base3=f'http://127.0.0.1:{port3}'
  (tmp/'config3.json').write_text(json.dumps({'data_dir':str(tmp/'data3'),'bind':f'127.0.0.1:{port3}','limits':{'workers':1,'timeout_seconds':30,'queued_jobs':10,'upload_bytes':131072,'disk_bytes':20000}}))
  p3=subprocess.Popen([str(BIN),'serve','--config',str(tmp/'config3.json')],env=env,stdout=subprocess.DEVNULL,stderr=open(tmp/'server3.log','a'))
  for _ in range(100):
   try:
    if request('/health/live',token=None,root=base3)[0]==200:break
   except OSError:pass
   time.sleep(.05)
  _,j=upload('digital.pdf',token=None,root=base3);stored=j['id'];wait(stored,root=base3)
  check('cache hits ignore the disk quota',upload('digital.pdf',token=None,root=base3)[1]['cached'] is True)
  check('tenant disk quota rejects new storage',upload('mixed.pdf',token=None,root=base3)[0]==507)
  p3.send_signal(signal.SIGINT);p3.wait(timeout=10);p3=None
 finally:
  if sys.exc_info()[0]:
   print((tmp/'server.log').read_text());
   for f in (tmp/'data/jobs').glob('*/worker.log'):print(f, f.read_text())
  if p3 and p3.poll() is None:p3.send_signal(signal.SIGINT);p3.wait(timeout=10)
  if p2 and p2.poll() is None:p2.send_signal(signal.SIGINT);p2.wait(timeout=10)
  if p and p.poll() is None:p.send_signal(signal.SIGINT);p.wait(timeout=10)
 (ROOT/'docs/e2e-results.json').write_text(json.dumps({'checks':results,'count':len(results),'date':time.strftime('%Y-%m-%d'),'platform':sys.platform},indent=2))
 print(f'{len(results)} real API acceptance checks passed')
