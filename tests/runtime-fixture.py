import json,sys,time
from http.server import HTTPServer,BaseHTTPRequestHandler
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_POST(self):
  data=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
  if len(sys.argv)>2:
   from pathlib import Path
   with Path(sys.argv[2]).open('a',encoding='utf-8') as log:
    log.write(json.dumps({'format':data.get('format'),'image_count':sum(len(m.get('images',[])) for m in data.get('messages',[]))})+'\n')
  self.send_response(200);self.send_header('Content-Type','application/x-ndjson');self.end_headers()
  if data.get('model')=='slow':time.sleep(15)
  self.wfile.write(b'{"message":{"content":"Managed runtime answer"},"done":true,"eval_count":4,"eval_duration":1000000000}\n')
 def do_GET(self):
  self.send_response(200);self.end_headers();self.wfile.write(b'{"models":[]}')
HTTPServer(('127.0.0.1',int(sys.argv[1])),Handler).serve_forever()
