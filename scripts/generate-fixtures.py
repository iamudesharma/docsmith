#!/usr/bin/env python3
"""Generate small, deterministic synthetic fixtures; Pillow is a development dependency only."""
import pathlib,zipfile,base64,zlib,urllib.parse,io,json,random
from PIL import Image,ImageDraw,ImageFont
D=pathlib.Path(__file__).resolve().parents[1]/'fixtures';D.mkdir(exist_ok=True)
def write(name,data): (D/name).write_bytes(data.encode() if isinstance(data,str) else data)
def package(name,entries):
 with zipfile.ZipFile(D/name,'w',zipfile.ZIP_DEFLATED) as z:
  for k,v in entries.items():z.writestr(zipfile.ZipInfo(k,(2026,10,11,0,0,0)),v)
write('quoted.csv','name,notes,number\nAda,"first, second",0012\nLin,"two\nlines",3.5\n')
write('flexible.tsv','one\ttwo\nthree\n')
write('document.md','# Heading\n\nNative text works offline.\n\n- First\n- Second\n')
write('document.json','{"items":[1,true,"text"],"unicode":"नमस्ते"}')
write('document.xml','<root><item>Native XML</item></root>')
write('unsafe.xml','<!DOCTYPE x [<!ENTITY leak SYSTEM "file:///etc/passwd">]><x>&leak;</x>')
write('document.html','<h1>Heading</h1><p>Offline HTML <a href="https://invalid.example">link</a></p><table><tr><th colspan="2">Header</th></tr><tr><td>A</td><td>B</td></tr></table><img src="https://invalid.example/p.png" alt="external image">')
write('message.eml','From: ada@example.test\nTo: lin@example.test\nSubject: Fixture\nMIME-Version: 1.0\nContent-Type: multipart/mixed; boundary=test\n\n--test\nContent-Type: text/plain; charset=utf-8\n\nEmail text\n--test\nContent-Type: application/octet-stream\nContent-Transfer-Encoding: base64\n\nYWJj\n--test--\n')
write('document.rtf','{\\rtf1\\ansi Native RTF\\par Second paragraph}')
model='<mxGraphModel><root><mxCell id="0"/><mxCell id="1" parent="0"/><mxCell id="users" value="Users" vertex="1" parent="1"><mxGeometry x="10" y="20" width="100" height="60"/></mxCell><mxCell id="orders" value="Orders" vertex="1" parent="1"/><mxCell id="edge" edge="1" source="orders" target="users" value="belongs to" parent="1"/></root></mxGraphModel>'
write('diagram.drawio','<mxfile><diagram name="ER">'+model+'</diagram></mxfile>')
compress=zlib.compressobj(wbits=-15);b=compress.compress(urllib.parse.quote(model,safe="!'()*-._~").encode())+compress.flush()
write('compressed.drawio','<mxfile><diagram name="ER">'+base64.b64encode(b).decode()+'</diagram></mxfile>')
import html
write('diagram.svg','<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" content="'+html.escape('<mxfile><diagram>'+model+'</diagram></mxfile>',quote=True)+'"><g transform="translate(2,3)"><text x="10" y="20">Users</text><path d="M0 0L10 20"/></g></svg>')
write('flow.mmd','flowchart TD\nA[Start]\nB[Finish]\nA --> B\nsubgraph unsupported\n')
write('flow.dot','digraph G {\nA [label="Start"];\nA -> B;\nsubgraph cluster_0 {\n}\n}\n')
write('flow.puml','@startuml\nA -> B\n@enduml\n')
write('schema.dbml','Table users {\n id int [pk]\n name varchar\n}\nTable orders {\n user_id int [ref: > users.id]\n}\nRef: orders.user_id > users.id\n')
font=ImageFont.truetype('/System/Library/Fonts/Supplemental/Arial.ttf',48)
img=Image.new('RGB',(1200,360),'white');draw=ImageDraw.Draw(img);draw.text((40,80),'Invoice TOTAL 12345',font=font,fill='black');img.save(D/'scan.png')
img.rotate(90,expand=True).save(D/'rotated.png')
noisy=img.copy();rand=random.Random(1)
for _ in range(1500):noisy.putpixel((rand.randrange(1200),rand.randrange(360)),(128,128,128))
noisy.save(D/'noisy.png')
# PNG retains explicit draw.io source in a standards-compliant tEXt chunk.
from PIL.PngImagePlugin import PngInfo
metadata=PngInfo();metadata.add_text('mxfile','<mxfile><diagram>'+model+'</diagram></mxfile>');img.save(D/'diagram.drawio.png',pnginfo=metadata)
image=(D/'scan.png').read_bytes()
w='http://schemas.openxmlformats.org/wordprocessingml/2006/main'
package('document.docx',{'word/document.xml':f'<w:document xmlns:w="{w}"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Word heading</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>Merged cell</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:pPr><w:sectPr/></w:pPr><w:r><w:t>Section one</w:t></w:r></w:p><w:p><w:r><w:t>Section two</w:t></w:r></w:p></w:body></w:document>','word/media/image1.png':image,'word/_rels/document.xml.rels':'<Relationships><Relationship Id="r1" Target="media/image1.png"/></Relationships>','docProps/core.xml':'<core><title>Fixture Word</title></core>'})
s='http://schemas.openxmlformats.org/spreadsheetml/2006/main';r='http://schemas.openxmlformats.org/officeDocument/2006/relationships'
package('workbook.xlsx',{'_rels/.rels':'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Target="xl/workbook.xml" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"/></Relationships>','[Content_Types].xml':'<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/></Types>','xl/workbook.xml':f'<workbook xmlns="{s}" xmlns:r="{r}"><sheets><sheet name="Data" sheetId="1" r:id="r1"/></sheets></workbook>','xl/_rels/workbook.xml.rels':'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Target="worksheets/sheet1.xml" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet"/></Relationships>','xl/worksheets/sheet1.xml':f'<worksheet xmlns="{s}"><dimension ref="A1:B3"/><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Header</t></is></c></row><row r="2"><c r="A2"><v>2</v></c><c r="B2"><v>3</v></c></row><row r="3"><c r="A3"><f>SUM(A2:B2)</f><v>5</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="A1:B1"/></mergeCells></worksheet>'})
# XLSM: identical workbook plus a VBA project part; macro code must never be executed or inspected.
with zipfile.ZipFile(D/'workbook.xlsx') as z:entries={k:z.read(k) for k in z.namelist()}
entries['xl/vbaProject.bin']=b'Attribute VB_Name = "Module1"\r\nSub AutoOpen()\r\n MsgBox "MACRO_SENTINEL_7f3a"\r\nEnd Sub\r\n'
entries['xl/_rels/workbook.xml.rels']=entries['xl/_rels/workbook.xml.rels'].replace(b'</Relationships>',b'<Relationship Id="r2" Target="vbaProject.bin" Type="http://schemas.microsoft.com/office/2006/relationships/vbaProject"/></Relationships>')
entries['[Content_Types].xml']=entries['[Content_Types].xml'].replace(b'spreadsheetml.sheet.main+xml',b'vnd.ms-excel.sheet.macroEnabled.main+xml',1).replace(b'</Types>',b'<Default Extension="bin" ContentType="application/vnd.ms-office.vbaProject"/></Types>',1)
package('workbook.xlsm',entries)
p='http://schemas.openxmlformats.org/presentationml/2006/main';a='http://schemas.openxmlformats.org/drawingml/2006/main'
package('presentation.pptx',{'ppt/presentation.xml':f'<p:presentation xmlns:p="{p}" xmlns:r="{r}"><p:sldIdLst><p:sldId id="256" r:id="r2"/><p:sldId id="257" r:id="r1"/></p:sldIdLst></p:presentation>','ppt/_rels/presentation.xml.rels':'<Relationships><Relationship Id="r1" Target="slides/slide1.xml"/><Relationship Id="r2" Target="slides/slide2.xml"/></Relationships>','ppt/slides/slide1.xml':f'<p:sld xmlns:p="{p}" xmlns:a="{a}"><p:sp><a:p><a:r><a:t>Second in source order</a:t></a:r></a:p></p:sp></p:sld>','ppt/slides/slide2.xml':f'<p:sld xmlns:p="{p}" xmlns:a="{a}"><p:sp><a:p><a:r><a:t>First in source order</a:t></a:r></a:p></p:sp></p:sld>'})
odf='urn:oasis:names:tc:opendocument:xmlns:office:1.0';text='urn:oasis:names:tc:opendocument:xmlns:text:1.0';table='urn:oasis:names:tc:opendocument:xmlns:table:1.0'
package('document.odt',{'mimetype':'application/vnd.oasis.opendocument.text','content.xml':f'<office:document-content xmlns:office="{odf}" xmlns:text="{text}"><office:body><office:text><text:h>ODT heading</text:h><text:p>ODT paragraph</text:p></office:text></office:body></office:document-content>'})
package('workbook.ods',{'META-INF/manifest.xml':'<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/></manifest:manifest>','mimetype':'application/vnd.oasis.opendocument.spreadsheet','content.xml':f'<office:document-content xmlns:office="{odf}" xmlns:text="{text}" xmlns:table="{table}"><office:body><office:spreadsheet><table:table table:name="Data"><table:table-row><table:table-cell office:value-type="float" office:value="42"><text:p>42</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>'})
package('book.epub',{'META-INF/container.xml':'<container><rootfiles><rootfile full-path="OPS/book.opf"/></rootfiles></container>','OPS/book.opf':'<package><manifest><item id="ch" href="chapter.xhtml"/></manifest><spine><itemref idref="ch"/></spine></package>','OPS/chapter.xhtml':'<html><body><h1>EPUB chapter</h1><p>EPUB content</p></body></html>'})
package('unsafe.docx',{'../outside.txt':'must never write'})
package('diagram.vsdx',{'visio/pages/page1.xml':'<Page><Shapes><Shape ID="1"/></Shapes></Page>'})
# Minimal PDF writer, exact source text and image placement known.
def pdf(name,pages):
 objects=[b'<< /Type /Catalog /Pages 2 0 R >>',b'']
 objects.append(b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>')
 jpeg=io.BytesIO();img.save(jpeg,format='JPEG');data=jpeg.getvalue()
 objects.append(b'<< /Type /XObject /Subtype /Image /Width 1200 /Height 360 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length '+str(len(data)).encode()+b' >>\nstream\n'+data+b'\nendstream')
 ids=[]
 for text,image_flag in pages:
  content=(f'BT /F1 18 Tf 40 750 Td ({text}) Tj ET\n' if text else '').encode()
  if text=='__COLUMNS__':content=b''.join(f'BT /F1 14 Tf {x} {700-20*r} Td ({label} {r+1}) Tj ET\n'.encode() for x,label in [(40,'Left'),(320,'Right')] for r in range(4))
  if text=='__TABLE__':content=b''.join(f'BT /F1 14 Tf {40+180*c} {700-20*r} Td ({value}) Tj ET\n'.encode() for r,row in enumerate([['Name','Amount'],['Ada','12'],['Lin','34']]) for c,value in enumerate(row))
  if image_flag:content+=b'q 540 0 0 162 30 450 cm /Im1 Do Q\n'
  pid=len(objects)+1;cid=pid+1;ids.append(pid)
  objects.append(f'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> /XObject << /Im1 4 0 R >> >> /Contents {cid} 0 R >>'.encode())
  objects.append(b'<< /Length '+str(len(content)).encode()+b' >>\nstream\n'+content+b'endstream')
 objects[1]=f'<< /Type /Pages /Count {len(ids)} /Kids ['.encode()+b' '.join(f'{i} 0 R'.encode() for i in ids)+b'] >>'
 out=bytearray(b'%PDF-1.4\n');offsets=[0]
 for i,obj in enumerate(objects,1):offsets.append(len(out));out+=f'{i} 0 obj\n'.encode()+obj+b'\nendobj\n'
 start=len(out);out+=f'xref\n0 {len(offsets)}\n0000000000 65535 f \n'.encode()
 for offset in offsets[1:]:out+=f'{offset:010d} 00000 n \n'.encode()
 out+=f'trailer << /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n'.encode();write(name,out)
pdf('digital.pdf',[('Native page ONE',False),('Native page TWO',False)])
pdf('scanned.pdf',[('',True)])
pdf('mixed.pdf',[('Digital heading',True)])
pdf('hidden-ocr.pdf',[('Invoice TOTAL 12345',True)])
pdf('multicolumn.pdf',[('__COLUMNS__',False)])
pdf('table.pdf',[('__TABLE__',False)])
# Multi-page PDF with images on specific pages, for batch image-attribution tests.
def pdf_with_images(name,pages):
 red=io.BytesIO();Image.new('RGB',(60,40),'red').save(red,format='JPEG');d1=red.getvalue()
 blue=io.BytesIO();Image.new('RGB',(60,40),'blue').save(blue,format='JPEG');d2=blue.getvalue()
 objects=[b'<< /Type /Catalog /Pages 2 0 R >>',b'',b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',b'<< /Type /XObject /Subtype /Image /Width 60 /Height 40 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length '+str(len(d1)).encode()+b' >>\nstream\n'+d1+b'\nendstream',b'<< /Type /XObject /Subtype /Image /Width 60 /Height 40 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length '+str(len(d2)).encode()+b' >>\nstream\n'+d2+b'\nendstream']
 ids=[]
 for text,which in pages:
  content=(f'BT /F1 18 Tf 40 750 Td ({text}) Tj ET\n').encode()
  if which:content+=f'q 60 0 0 40 40 400 cm /Im{which} Do Q\n'.encode()
  pid=len(objects)+1;cid=pid+1;ids.append(pid)
  objects.append(f'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> /XObject << /Im1 4 0 R /Im2 5 0 R >> >> /Contents {cid} 0 R >>'.encode())
  objects.append(b'<< /Length '+str(len(content)).encode()+b' >>\nstream\n'+content+b'endstream')
 objects[1]=f'<< /Type /Pages /Count {len(ids)} /Kids ['.encode()+b' '.join(f'{i} 0 R'.encode() for i in ids)+b'] >>'
 out=bytearray(b'%PDF-1.4\n');offsets=[0]
 for i,obj in enumerate(objects,1):offsets.append(len(out));out+=f'{i} 0 obj\n'.encode()+obj+b'\nendobj\n'
 start=len(out);out+=f'xref\n0 {len(offsets)}\n0000000000 65535 f \n'.encode()
 for offset in offsets[1:]:out+=f'{offset:010d} 00000 n \n'.encode()
 out+=f'trailer << /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n'.encode();write(name,out)
pdf_with_images('pages-images.pdf',[('Page one',0),('Page two',1),('Page three',0),('Page four',0),('Page five',2),('Page six',0)])
write('corrupt.pdf',b'%PDF-1.7\ncorrupt')
write('unsupported.doc',b'legacy binary placeholder')
write('ground-truth.json',json.dumps({'scan.png':'Invoice TOTAL 12345','noisy.png':'Invoice TOTAL 12345','rotated.png':'Invoice TOTAL 12345','diagram':{'nodes':['users','orders'],'edges':[['orders','users']]},'raster_er':{'entities':['users','orders'],'edge_count':1},'synthetic':True},indent=2))

# Complete package roots/spine wrappers so external engines can inspect the same fixtures.
for name,root_part,content_type in [('document.docx','word/document.xml','application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml'),('presentation.pptx','ppt/presentation.xml','application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml')]:
 with zipfile.ZipFile(D/name) as z:entries={k:z.read(k) for k in z.namelist()}
 entries['_rels/.rels']=f'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Target="{root_part}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"/></Relationships>'
 entries['[Content_Types].xml']=f'<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/{root_part}" ContentType="{content_type}"/></Types>'
 if name.endswith('pptx'):
  for k,v in list(entries.items()):
   if k.startswith('ppt/slides/') and k.endswith('.xml'):
    text=v.decode();text=text.replace('<p:sp>','<p:cSld><p:spTree><p:sp><p:txBody>').replace('</p:sp>','</p:txBody></p:sp></p:spTree></p:cSld>');entries[k]=text
 package(name,entries)
# Source metadata embedded inside a PDF: the original draw.io graph is retained.
b=(D/'digital.pdf').read_bytes();source=b'<mxfile><diagram>'+model.encode()+b'</diagram></mxfile>';stream=zlib.compress(source)
objects=[b'<< /Type /Catalog /Pages 2 0 R /Metadata 5 0 R >>',b'<< /Type /Pages /Count 1 /Kids [3 0 R] >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>',b'<< /Length 0 >>\nstream\n\nendstream',b'<< /Type /Metadata /Subtype /XML /Filter /FlateDecode /Length '+str(len(stream)).encode()+b' >>\nstream\n'+stream+b'\nendstream']
out=bytearray(b'%PDF-1.4\n');offsets=[0]
for i,obj in enumerate(objects,1):offsets.append(len(out));out+=f'{i} 0 obj\n'.encode()+obj+b'\nendobj\n'
start=len(out);out+=f'xref\n0 {len(offsets)}\n0000000000 65535 f \n'.encode()
for offset in offsets[1:]:out+=f'{offset:010d} 00000 n \n'.encode()
out+=f'trailer << /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n'.encode();write('embedded-diagram.pdf',out)

# Actual raster ER figure, independent of retained source XML.
er=Image.new('RGB',(1000,600),'white');draw=ImageDraw.Draw(er);font=ImageFont.truetype('/System/Library/Fonts/Supplemental/Arial.ttf',36)
for box,title,fields in [((50,100,400,400),'Users',['id PK','name']),((600,100,950,400),'Orders',['id PK','user_id FK'])]:
 draw.rectangle(box,outline='black',width=4);draw.text((box[0]+25,box[1]+25),title,font=font,fill='black')
 for i,field in enumerate(fields):draw.text((box[0]+25,box[1]+110+70*i),field,font=font,fill='black')
draw.line((600,260,400,260),fill='black',width=4);draw.polygon([(400,260),(420,250),(420,270)],fill='black');draw.text((450,210),'1:N',font=font,fill='black');er.save(D/'raster-er.png')
try:
 hindi='नमस्ते भारत';im=Image.new('RGB',(1200,300),'white');font=ImageFont.truetype('/System/Library/Fonts/Kohinoor.ttc',64);ImageDraw.Draw(im).text((40,80),hindi,font=font,fill='black');im.save(D/'hindi.png')
 truth=json.loads((D/'ground-truth.json').read_text());truth['hindi.png']=hindi;(D/'ground-truth.json').write_text(json.dumps(truth,indent=2))
except OSError:pass

# Legacy BIFF8 .xls built directly (no external tools): A1 "Header", B2 3.0, A3 formula
# with literal-5 tokens and cached result 5.0. Container is a minimal OLE2/CFB file.
import struct as _struct
def _rec(t,p):return _struct.pack('<HH',t,len(p))+p
def _bof(dt):return _rec(0x0809,_struct.pack('<HHHHII',0x0600,dt,0x0DBB,0x07CC,0,0))
def _boundsheet(pos):
 name='Data'.encode('utf-16-le');payload=_struct.pack('<I',pos)+bytes([0,0])+bytes([4,1])+name;return _rec(0x0085,payload)
def _sst():
 payload=_struct.pack('<II',1,1)+_struct.pack('<H',6)+bytes([1])+'Header'.encode('utf-16-le');return _rec(0x00FC,payload)
def _dimensions():return _rec(0x0200,_struct.pack('<IIHH',0,2,0,1)+b'\x00\x00')
def _number(row,col,value):return _rec(0x0203,_struct.pack('<HHH',row,col,0)+_struct.pack('<d',value))
def _labelsst(row,col,index):return _rec(0x00FD,_struct.pack('<HHHI',row,col,0,index))
def _formula(row,col,cached):
 tokens=b'\x1e'+_struct.pack('<H',5)
 payload=_struct.pack('<HHH',row,col,0)+_struct.pack('<d',cached)+_struct.pack('<H',0)+_struct.pack('<I',0)+_struct.pack('<H',len(tokens))+tokens
 return _rec(0x0006,payload)
def _dir_entry(name,obj_type,start,size,child=0xFFFFFFFF):
 entry=name.encode('utf-16-le')+b'\x00\x00'
 return _struct.pack('<64sHBBIII16sIQQIQ',entry.ljust(64,b'\x00'),len(entry),obj_type,1,0xFFFFFFFF,0xFFFFFFFF,child,b'\x00'*16,0,0,0,start,size)
def _ole2(stream):
 mini_count=(len(stream)+63)//64;mini_bytes=mini_count*64
 fat=[0xFFFFFFFF]*128;fat[0]=0xFFFFFFFD;fat[1]=0xFFFFFFFE;fat[2]=0xFFFFFFFE;fat[3]=0xFFFFFFFD
 header=bytearray(512);header[0:8]=b'\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1'
 header[24:26]=_struct.pack('<H',0x003E);header[26:28]=_struct.pack('<H',3);header[28:30]=_struct.pack('<H',0xFFFE)
 header[30:32]=_struct.pack('<H',0x0009);header[32:34]=_struct.pack('<H',0x0006)
 header[44:48]=_struct.pack('<I',1);header[48:52]=_struct.pack('<I',2);header[56:60]=_struct.pack('<I',4096)
 header[60:64]=_struct.pack('<I',3);header[64:68]=_struct.pack('<I',1);header[68:72]=_struct.pack('<I',0xFFFFFFFE)
 header[76:80]=_struct.pack('<I',0);header[80:512]=b'\xff'*(512-80)
 entries=_dir_entry('Root Entry',5,1,mini_bytes,child=1)+_dir_entry('Workbook',2,0,len(stream))
 mini_fat=[0xFFFFFFFF]*128
 for i in range(mini_count-1):mini_fat[i]=i+1
 mini_fat[mini_count-1]=0xFFFFFFFE
 return b''.join([bytes(header),_struct.pack('<128I',*fat),stream.ljust(512,b'\x00'),entries.ljust(512,b'\x00'),_struct.pack('<128I',*mini_fat)])
placeholder=_bof(0x0005)+_boundsheet(0)+_sst()+_rec(0x000A,b'')
workbook=_bof(0x0005)+_boundsheet(len(placeholder))+_sst()+_rec(0x000A,b'')+_bof(0x0010)+_dimensions()+_labelsst(0,0,0)+_number(1,1,3.0)+_formula(2,0,5.0)+_rec(0x000A,b'')
write('workbook.xls',_ole2(workbook))
print('Generated',len(list(D.iterdir())),'fixtures')
