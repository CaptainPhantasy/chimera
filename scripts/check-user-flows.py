#!/usr/bin/env python3
"""Exercise the actual CHIMERA executable against a controlled local model.

Uses disposable folders and an operating-system-assigned loopback test port.
It never calls a hosted model or reads the user's model credentials.
Keeps its folders and report for inspection. Not a substitute for human testing.
"""
import argparse
import codecs
import fcntl
import http.server
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time

parser = argparse.ArgumentParser()
parser.add_argument('binary', type=Path)
parser.add_argument('--tui-only', action='store_true', help='run the terminal controls and display checks only')
args = parser.parse_args()
binary = args.binary.resolve(strict=True)
binary_hash=hashlib.sha256(binary.read_bytes()).hexdigest()
root = Path(tempfile.mkdtemp(prefix='chimera-user-flows-'))
print(json.dumps({'workspace':str(root),'binary_sha256':binary_hash,'tui_only':args.tui_only}),flush=True)
requests = []
requests_lock = threading.Lock()
stream_part_sent = threading.Event()
stream_continue = threading.Event()
long_answer='ANSWER_BEGIN café 世界 🦋\n'+'\n'.join(f'answer row {n:03} — café 世界 🦋' for n in range(1,61))+'\nANSWER_END'
long_error='ERROR_BEGIN café 世界 🦋\n'+'\n'.join(f'error row {n:03} — café 世界 🦋' for n in range(1,61))+'\nERROR_END'

class Model(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args): pass
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        with requests_lock:
            requests.append(body)
            request_number=len(requests)
        prompt = next(m['content'] for m in reversed(body['messages']) if m['role'] == 'user')
        if prompt == 'server failure':
            self.send_response(503); self.end_headers(); self.wfile.write(b'{"error":{"message":"controlled outage"}}'); return
        if prompt == 'long screen error':
            self.send_response(503); self.end_headers(); self.wfile.write(json.dumps({'error':{'message':long_error}},ensure_ascii=False).encode()); return
        if prompt == 'stall':
            time.sleep(3); return
        self.send_response(200)
        self.send_header('Content-Type','text/event-stream')
        self.end_headers()
        def data(value):
            self.wfile.write(('data: '+json.dumps(value,ensure_ascii=False)+'\n\n').encode()); self.wfile.flush()
        if prompt == 'stream now':
            raw=('data: '+json.dumps({'choices':[{'delta':{'content':'first café 🦋'},'finish_reason':None}]},ensure_ascii=False)+'\n\n').encode()
            butterfly=raw.index('🦋'.encode())
            self.wfile.write(raw[:butterfly+1]); self.wfile.flush()
            self.wfile.write(raw[butterfly+1:]); self.wfile.flush()
            stream_part_sent.set()
            if not stream_continue.wait(10): return
            data({'choices':[{'delta':{'content':' second'},'finish_reason':None}]})
        elif prompt in ('try write','approved write','remote write','remote denied','screen write','interactive write','detail write','CONTROL plan write','CONTROL denied write') and body['messages'][-1]['role'] != 'tool':
            data({'choices':[{'delta':{'tool_calls':[{'index':0,'id':'write-1','type':'function','function':{'name':'write_file','arguments':json.dumps({'path':{'remote write':'remote.txt','remote denied':'denied.txt','screen write':'screen.txt','interactive write':'interactive.txt','detail write':'detail.txt','CONTROL plan write':'planned-write.txt','CONTROL denied write':'screen-denied.txt'}.get(prompt,'result.txt'),'content':'human approved edit\n'})}}]},'finish_reason':'tool_calls'}]})
            self.wfile.write(b'data: [DONE]\n\n'); self.wfile.flush(); return
        elif prompt == 'external read' and body['messages'][-1]['role'] != 'tool':
            data({'choices':[{'delta':{'tool_calls':[{'index':0,'id':'external-1','type':'function','function':{'name':'mcp__fixture__read','arguments':'{}'}}]},'finish_reason':'tool_calls'}]})
            self.wfile.write(b'data: [DONE]\n\n'); self.wfile.flush(); return
        elif prompt == 'empty answer': pass
        elif prompt == 'cut off': data({'choices':[{'delta':{'content':'unfinished text'},'finish_reason':'length'}]})
        elif prompt == 'long screen answer': data({'choices':[{'delta':{'content':long_answer},'finish_reason':None}]})
        else:
            old = any(m.get('content') == 'remember apricot' for m in body['messages'][:-1])
            reply = 'apricot remembered' if prompt == 'what fruit?' and old else 'controlled answer saved'
            if prompt.startswith('CONTROL '): reply=f'CONTROL_REPLY_{request_number}'
            data({'choices':[{'delta':{'content':reply},'finish_reason':None}]})
        if prompt != 'cut off': data({'choices':[{'delta':{},'finish_reason':'stop'}]})
        data({'choices':[],'usage':{'prompt_tokens':12,'completion_tokens':5,'total_tokens':17}})
        self.wfile.write(b'data: [DONE]\n\n'); self.wfile.flush()

server = http.server.ThreadingHTTPServer(('127.0.0.1',0),Model)
server.daemon_threads = True
threading.Thread(target=server.serve_forever,daemon=True).start()
config = root/'settings.json'
config.write_text(json.dumps({'provider':{'name':'controlled','base_url':f'http://127.0.0.1:{server.server_port}/v1','model':'controlled-model','api_key':'FAKE_SECRET_DO_NOT_EXPORT','request_timeout_secs':10,'idle_timeout_secs':5},'agent':{'max_iterations':4}}))
env = {k:v for k,v in os.environ.items() if not k.startswith('CHIMERA_') and k not in ('OPENAI_API_KEY','ANTHROPIC_API_KEY','NO_COLOR')}
env.update(CHIMERA_CONFIG=str(config),TERM='xterm-256color',COLORTERM='truecolor')
results = []
owned_processes = []
owned_terminals = []
service_started = False
cleanup = []
failure = None

def save_report():
    report={'binary':str(binary),'binary_sha256':binary_hash,'workspace':str(root),'tui_only':args.tui_only,
        'results':results,'failure':failure,'cleanup':cleanup}
    (root/'check-report.json').write_text(json.dumps(report,indent=2)+'\n')

def check(name, condition, details=''):
    results.append({'check':name,'pass':bool(condition),'details':details if not condition else ''})
    save_report()
    if not condition: raise AssertionError(f'{name}: {details}')

def cli(*argv, expected=0):
    global service_started
    result = subprocess.run([str(binary),'--format','json',*argv],cwd=root,env=env,capture_output=True,text=True,timeout=15)
    if argv==('up',) and result.returncode==0:service_started=True
    if argv==('down',) and result.returncode==0:service_started=False
    check('exit '+' '.join(argv),result.returncode==expected,result.stdout+result.stderr if result.returncode!=expected else '')
    lines=[json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    return lines

def controlling_terminal():
    os.setsid()
    fcntl.ioctl(0,termios.TIOCSCTTY,0)

class Terminal:
    def __init__(self, argv, columns=100, rows=30):
        self.master,self.slave=pty.openpty()
        self.original=termios.tcgetattr(self.slave)
        fcntl.ioctl(self.slave,termios.TIOCSWINSZ,struct.pack('HHHH',rows,columns,0,0))
        self.proc=subprocess.Popen([str(binary),*argv],cwd=root,env=env,stdin=self.slave,stdout=self.slave,stderr=self.slave,preexec_fn=controlling_terminal)
        owned_processes.append(self.proc)
        self.output=b''
        self.columns,self.rows=columns,rows
        self.events=[{'type':'resize','columns':columns,'rows':rows}]
        self.decoder=codecs.getincrementaldecoder('utf8')('replace')
        self.closed=False
        self.eof=False
        self.fullscreen=argv[0]=='tui' or (argv[0]=='run' and '--interactive' in argv) or argv[:2]==['session','resume']
        owned_terminals.append(self)
    def record(self,data):
        self.output+=data
        text=self.decoder.decode(data)
        if text:self.events.append({'type':'write','data':text})
    def read_until(self, text, limit=15, start=0):
        expected=text.encode()
        deadline=time.monotonic()+limit
        while expected not in self.output[start:] and time.monotonic()<deadline:
            if self.closed or self.eof:break
            self.drain(.1)
        check('terminal shows '+text, expected in self.output[start:],self.output[-4000:].decode(errors='replace'))
    def drain(self,timeout=0):
        if self.closed or self.eof:return
        if select.select([self.master],[],[],timeout)[0]:
            try:data=os.read(self.master,65536)
            except OSError:self.eof=True;return
            if data:self.record(data)
            else:self.eof=True
    def settle(self,seconds=.15):
        deadline=time.monotonic()+seconds
        while not self.closed and not self.eof and time.monotonic()<deadline:self.drain(.02)
    def redraw(self):
        # The runner handles queued keys one at a time. Observe both actual
        # screens instead of guessing when earlier typing/scrolling finished.
        # Opening/closing help retains the input, scroll position and theme.
        start=len(self.output);self.send('\x1bOP')
        self.read_until('CHIMERA controls',start=start)
        start=len(self.output);self.send('\x1b')
        self.read_until('◆ CHIMERA',start=start)
    def snapshot(self,name,help_open=False):
        if not help_open:self.redraw()
        self.settle()
        target=root/(name+'.terminal.json')
        target.write_text(json.dumps(self.events,ensure_ascii=False)+'\n')
        (root/(name+'.ansi')).write_bytes(self.output)
        (root/(name+'.screen.json')).write_text(json.dumps({'events':str(target),'columns':self.columns,'rows':self.rows,'name':name})+'\n')
    def preserve(self):
        (root/f'terminal-{self.proc.pid}.terminal.json').write_text(json.dumps(self.events,ensure_ascii=False)+'\n')
        (root/f'terminal-{self.proc.pid}.ansi').write_bytes(self.output)
    def close(self):
        if self.closed:return
        self.closed=True
        for descriptor in (self.master,self.slave):
            try:os.close(descriptor)
            except OSError:pass
    def wait_with_output(self,timeout):
        deadline=time.monotonic()+timeout
        while self.proc.poll() is None and time.monotonic()<deadline:
            if self.closed or self.eof:time.sleep(.02)
            else:self.drain(.05)
        self.settle(.05)
        return self.proc.poll()
    def send(self,text): os.write(self.master,text.encode())
    def approve_next(self):
        self.read_until('Type yes or no:')
        self.output=b''
        self.send('yes\n')
    def resize(self,columns,rows):
        self.settle()
        fcntl.ioctl(self.slave,termios.TIOCSWINSZ,struct.pack('HHHH',rows,columns,0,0))
        self.proc.send_signal(signal.SIGWINCH)
        self.columns,self.rows=columns,rows
        self.events.append({'type':'resize','columns':columns,'rows':rows})
    def finish(self,code=0):
        actual=self.wait_with_output(15)
        self.preserve()
        check('terminal exit',actual==code,f'expected {code}, actual {actual}; '+self.output[-2000:].decode(errors='replace'))
        check('terminal restored',termios.tcgetattr(self.master)==self.original)
        if self.fullscreen:
            plain=re.sub(rb'\x1b\[[0-?]*[ -/]*[@-~]',b'',self.output)
            raw_log=re.search(rb'\d{4}-\d\d-\d\dT[0-9:.]+Z\s+(?:TRACE|DEBUG|INFO|WARN|ERROR)\s+chimera_',plain)
            check('fullscreen output has no raw tracing records',raw_log is None,plain[max(0,raw_log.start()-50):raw_log.start()+300].decode(errors='replace') if raw_log else '')
        self.close()

def request_count():
    with requests_lock:return len(requests)

def wait_request(terminal,prompt,after):
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        with requests_lock:
            matches=[(n+1,row) for n,row in enumerate(requests) if n>=after and row['messages'][-1].get('content')==prompt]
        if matches:
            check('terminal submits exact input '+prompt[:60],True)
            return matches[0]
        terminal.settle(.05)
    with requests_lock:observed=[row['messages'][-1].get('content') for row in requests[after:]]
    check('terminal submits exact input '+prompt[:60],False,repr(observed))

def control_request(terminal,prompt,keys=None):
    after=request_count();start=len(terminal.output)
    terminal.send(prompt+'\r' if keys is None else keys)
    number,row=wait_request(terminal,prompt,after)
    # Terminal output is a screen diff: a new answer may redraw only its changed
    # digits. Verify the saved completion, then retain the real screen writes.
    deadline=time.monotonic()+15
    complete=False
    while time.monotonic()<deadline:
        state=saved_session(terminal.session_path)
        complete=not state['active'] and any(e.get('key')=='run_result' and e['payload'].get('final_answer')==f'CONTROL_REPLY_{number}' for e in state['events'])
        if complete:break
        terminal.settle(.05)
    check('typed request completes and is saved '+prompt[:60],complete)
    terminal.settle()
    return row

def saved_session(path):
    value=json.loads(path.read_text())
    return value.get('data',value)

def poll_until_exit(process,timeout):
    deadline=time.monotonic()+timeout
    while process.poll() is None and time.monotonic()<deadline:time.sleep(.02)
    return process.poll()

def cleanup_process(process,terminal=None):
    row={'pid':process.pid,'status':'started'}
    cleanup.append(row);save_report()
    try:
        if terminal:
            # Save and close the terminal before waiting: a child must not
            # remain blocked writing its final screen into an unread PTY.
            try:
                terminal.settle(.1)
                terminal.preserve()
            except Exception as error:row['trace_error']=str(error)
            finally:
                terminal.close()
        else:
            for name in ('stdout','stderr'):
                stream=getattr(process,name,None)
                if stream is None or stream.closed:continue
                tail=bytearray()
                try:
                    os.set_blocking(stream.fileno(),False)
                    while len(tail)<1024*1024:
                        try:chunk=os.read(stream.fileno(),65536)
                        except BlockingIOError:break
                        if not chunk:break
                        tail.extend(chunk)
                finally:
                    (root/f'process-{process.pid}-{name}.txt').write_bytes(tail)
                    stream.close()
        if process.poll() is None:
            row['terminated']=True;save_report();process.terminate()
            if poll_until_exit(process,2) is None:
                row['killed']=True;save_report();process.kill()
                if poll_until_exit(process,2) is None:
                    raise TimeoutError('owned child did not exit after bounded cleanup')
        row['status']='failed' if 'trace_error' in row else 'finished'
    except Exception as error:
        row['status']='failed';row['error']=str(error)
    finally:
        row['exit_code']=process.poll()
        if terminal:
            terminal.close()
            try:terminal.preserve()
            except Exception as error:
                row['status']='failed';row['trace_error']=str(error)
        save_report()

def exercise_terminal_controls():
    before=set((root/'.chimera/sessions').glob('*.json'))
    terminal=Terminal(['tui'])
    terminal.read_until('controlled-model')
    session_path=next(iter(set((root/'.chimera/sessions').glob('*.json'))-before))
    terminal.session_path=session_path
    terminal.send('\x1bOP');terminal.read_until('CHIMERA controls');terminal.snapshot('controls-help-dark',help_open=True)
    start=len(terminal.output);terminal.send('\x1b');terminal.read_until('◆ CHIMERA',start=start)
    start=len(terminal.output);terminal.send('/help\r');terminal.read_until('CHIMERA controls',start=start);terminal.snapshot('controls-slash-help',help_open=True)
    start=len(terminal.output);terminal.send('\x1b');terminal.read_until('◆ CHIMERA',start=start)

    control_request(terminal,'CONTROL ordinary café 世界 🦋?')
    pasted='CONTROL paste café 世界 🦋?\nsecond line'
    control_request(terminal,pasted,'\x1b[200~'+pasted+'\x1b[201~\r')
    control_request(terminal,'CONTROL alt\nline','CONTROL alt\x1b\rline\r')
    control_request(terminal,'CONTROL shift\nline','CONTROL shift\x1b[13;2uline\r')
    keys='CONTROL nav axc?'+'\x1b[H'+'\x1b[C'*len('CONTROL nav a')+'\x1b[3~b'+'\x1b[F\x1b[D\x7fc'+'\x1b[F\x1b[1;3Dx\x1b[1;3C tail'+'\x1b[D\x1b[C\r'
    control_request(terminal,'CONTROL nav xabc? tail',keys)
    control_request(terminal,'CONTROL history first')
    control_request(terminal,'CONTROL history second')
    control_request(terminal,'CONTROL history second repeat','\x1b[A\x1b[A\x1b[B repeat\r')
    long_input='CONTROL long '+'café 世界 🦋 '*160+'\nCONTROL long tail'
    terminal.send('\x1b[200~'+long_input+'\x1b[201~');terminal.snapshot('controls-long-input-dark')
    control_request(terminal,long_input,'\r')

    start=len(terminal.output);terminal.send('/not-a-command\r')
    terminal.read_until('Unknown command /not-a-command',start=start)
    terminal.send('/tools\r');terminal.settle();terminal.send('\x1b[5~'*20);terminal.snapshot('controls-tools-top')
    terminal.send('\x1b[6~'*20);terminal.settle()
    check('tools command displays real registered names',b'write_file' in terminal.output and b'read_file' in terminal.output)
    start=len(terminal.output);terminal.send('/sessions\r');terminal.read_until(session_path.stem,start=start)
    terminal.send('/model controlled-renamed\r');terminal.settle()
    start=len(terminal.output);terminal.send('/model\r');terminal.read_until('controlled-renamed',start=start)
    row=control_request(terminal,'CONTROL renamed model')
    check('model control changes actual provider request',row['model']=='controlled-renamed')
    terminal.send('/model controlled-model\r');terminal.settle()

    start=len(terminal.output);terminal.send('\x0c');terminal.read_until('48;2;245;244;242',start=start);terminal.snapshot('controls-light-100x30')
    check('Control+L applies the light background',b'48;2;245;244;242' in terminal.output[start:])
    terminal.resize(80,24);terminal.snapshot('controls-light-80x24')
    terminal.resize(160,45);terminal.snapshot('controls-light-160x45')
    start=len(terminal.output);terminal.send('/theme\r');terminal.read_until('48;2;24;23;27',start=start);terminal.snapshot('controls-dark-160x45')
    check('theme command applies the dark background',b'48;2;24;23;27' in terminal.output[start:])
    terminal.resize(100,30);terminal.snapshot('controls-dark-100x30')
    terminal.send('/clear\r');terminal.snapshot('controls-cleared')
    row=control_request(terminal,'CONTROL context after clear','/run CONTROL context after clear\r')
    check('clear retains actual saved conversation',any(m.get('content')==pasted for m in row['messages']))

    after=request_count();start=len(terminal.output);terminal.send('/plan CONTROL plan write\r')
    _,row=wait_request(terminal,'CONTROL plan write',after)
    terminal.read_until('unknown or unavailable tool',start=start)
    check('plan control removes mutating tools from actual request',not any(t['function']['name']=='write_file' for t in row['tools']))
    check('plan control changes no file',not (root/'planned-write.txt').exists())
    terminal.settle()

    start=len(terminal.output);terminal.send('/run CONTROL denied write\r')
    terminal.read_until('Use /approve or /deny',start=start);terminal.send('/deny\r')
    terminal.read_until('action denied or approval expired',start=start)
    check('deny control changes no file',not (root/'screen-denied.txt').exists())
    terminal.settle()

    start=len(terminal.output);terminal.send('detail write\r')
    terminal.read_until('Use /approve or /deny',start=start);terminal.send('/approve\r')
    terminal.read_until('controlled answer saved',start=start);terminal.settle()
    start=len(terminal.output);terminal.send('\t');terminal.read_until('recovery_copy',start=start)
    terminal.snapshot('controls-approved-tool-details')
    check('approved tool details show the actual result',b'"bytes":20' in terminal.output[start:] and (root/'detail.txt').read_text()=='human approved edit\n')
    terminal.send('\t');terminal.snapshot('controls-tool-collapsed')

    after=request_count();start=len(terminal.output);terminal.send('stall\r');wait_request(terminal,'stall',after)
    terminal.send('/cancel\r');terminal.read_until('cancelled by user',start=start);terminal.settle()
    state=saved_session(session_path)
    check('cancel control saves cancelled run outcome',any(e.get('key')=='run_outcome' and e['payload']['outcome']=='cancelled' for e in state['events']))
    terminal.send('/quit\r');terminal.finish()

    for name,prompt,begin,end in [('long-answer','long screen answer','ANSWER_BEGIN','ANSWER_END'),('long-error','long screen error','ERROR_BEGIN','ERROR_END')]:
        before=set((root/'.chimera/sessions').glob('*.json'))
        terminal=Terminal(['run','--interactive',prompt],80,24)
        terminal.read_until(end);terminal.snapshot(name+'-bottom-dark-80x24')
        start=len(terminal.output);terminal.send('\x1b[5~'*20);terminal.read_until(begin,start=start);terminal.snapshot(name+'-top-dark-80x24')
        start=len(terminal.output);terminal.send('\x1b[6~'*20);terminal.read_until(end,start=start)
        terminal.send('\x0c');terminal.snapshot(name+'-bottom-light-80x24')
        terminal.send('/exit\r');terminal.finish()
        if name=='long-error':
            error_path=next(iter(set((root/'.chimera/sessions').glob('*.json'))-before))
            state=saved_session(error_path)
            check('fullscreen provider error remains saved',not state['active'] and any(e.get('key')=='run_result' and 'ERROR_BEGIN' in str(e['payload'].get('error','')) for e in state['events']))

    before=set((root/'.chimera/sessions').glob('*.json'))
    terminal=Terminal(['tui']);terminal.read_until('controlled-model')
    cancelled_path=next(iter(set((root/'.chimera/sessions').glob('*.json'))-before))
    after=request_count();terminal.send('stall\r');wait_request(terminal,'stall',after)
    terminal.send('\x03');terminal.finish()
    state=saved_session(cancelled_path)
    check('Control+C saves cancelled run and leaves',not state['active'] and any(e.get('key')=='run_outcome' and e['payload']['outcome']=='cancelled' for e in state['events']))

try:
    exercise_terminal_controls()
    if args.tui_only:sys.exit(0)
    cli('status')
    cli('doctor')
    cli('tool','healthcheck','does-not-exist',expected=1)
    cli('--profile','unknown','status',expected=1)
    cli('approve','plan',expected=1)
    for prompt in ('server failure','empty answer','cut off'):
        rows=cli('run',prompt,expected=1)
        check('failure never has successful result '+prompt,not any(row.get('success') is True for row in rows))
    cli('plan','try write',expected=1)
    check('planning changed no project file',not (root/'result.txt').exists())
    cli('run','try write',expected=1)
    check('unapproved write changed no file',not (root/'result.txt').exists())
    terminal=Terminal(['run','approved write'])
    terminal.read_until('Type yes or no:')
    terminal.send('yes\n')
    terminal.read_until('Answer saved to session')
    terminal.finish()
    check('approved tool made actual file',(root/'result.txt').read_text()=='human approved edit\n')
    cli('policy','set','read-only')
    cli('run','try write',expected=1)
    cli('policy','set','supervised')
    cli('tool','disable','write_file')
    cli('run','try write',expected=1)
    cli('tool','enable','write_file')
    for prompt,decision,path in [('remote write','approve','remote.txt'),('remote denied','deny','denied.txt')]:
        pending=subprocess.Popen([str(binary),'--format','json','run',prompt],cwd=root,env={**env,'CHIMERA_WAIT_APPROVAL':'1'},stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        owned_processes.append(pending)
        action=None
        buffered=b''
        until=time.monotonic()+15
        while time.monotonic()<until:
            ready,_,_=select.select([pending.stdout],[],[],.1)
            if ready:
                buffered+=os.read(pending.stdout.fileno(),65536)
                while b'\n' in buffered:
                    line,buffered=buffered.split(b'\n',1)
                    row=json.loads(line)
                    if row.get('event')=='approval_required': action=row['id'];break
                if action: break
            if pending.poll() is not None: break
        check('exact pending action is visible '+prompt,action is not None)
        if decision=='approve': cli('approve','action',action)
        else: cli('deny',action)
        out,err=pending.communicate(timeout=15)
        check('remote decision affects running process '+prompt,pending.returncode==(0 if decision=='approve' else 1),out+err)
        check('remote decision controls file '+prompt,(root/path).exists()==(decision=='approve'))
        cli('approve','action',action,expected=1)
    for entry,prompt,path in [(['tui'],'screen write','screen.txt'),(['run','--interactive','interactive write'],'interactive write','interactive.txt')]:
        terminal=Terminal(entry)
        terminal.read_until('controlled-model')
        if entry==['tui']: terminal.send(prompt+'\r')
        terminal.read_until('Use /approve or /deny')
        terminal.send('/approve\r')
        terminal.read_until('controlled answer saved')
        terminal.send('/quit\r');terminal.finish()
        check('interactive entry makes approved file '+prompt,(root/path).read_text()=='human approved edit\n')

    terminal=Terminal(['run','stream now'])
    check('controlled first fragment sent',stream_part_sent.wait(10))
    terminal.read_until('first café 🦋')
    check('first text displayed before completion',not stream_continue.is_set())
    stream_continue.set()
    terminal.finish()
    terminal=Terminal(['run','stall'])
    terminal.read_until('Session ')
    # Cancel as soon as the advertised session exists: startup must already
    # have installed its signal listener, even before its first network wait.
    terminal.proc.send_signal(signal.SIGINT)
    terminal.finish(code=1)
    initial=cli('run','remember apricot')
    original=next(row['id'] for row in initial if row.get('event')=='session')
    child=cli('session','fork',original)[0]['session']
    exported=cli('trace','export',original)
    check('diagnostics remove fake secrets','FAKE_SECRET_DO_NOT_EXPORT' not in json.dumps(exported))
    check('fork has a separate identity',child!=original)
    terminal=Terminal(['session','resume',child])
    terminal.read_until('controlled-model')
    terminal.send('what fruit?\r')
    terminal.read_until('apricot remembered')
    terminal.send('/theme\r')
    for width,height in [(80,24),(160,45),(100,30)]:
        terminal.resize(width,height)
        time.sleep(.1)
    terminal.send('/quit\r')
    terminal.finish()
    terminal=Terminal(['run','--interactive','remember apricot'])
    terminal.read_until('controlled answer saved')
    terminal.send('/exit\r')
    terminal.finish()
    with requests_lock:
        history=requests[:]
    check('starting interactive request submitted once',sum(1 for r in history if r['messages'][-1].get('content')=='remember apricot')==2)
    terminal=Terminal(['tui'])
    terminal.read_until('controlled-model')
    terminal.send('remember apricot\r')
    terminal.read_until('controlled answer saved')
    terminal.send('/quit\r')
    terminal.finish()
    replay=cli('replay',original)
    check('replay contains saved conversation',any(m.get('content')=='remember apricot' for m in replay[0]['messages']))
    (root/'checkpoint.txt').write_text('before checkpoint\n')
    (root/'unrelated.txt').write_text('leave this alone\n')
    checkpoint=cli('checkpoint','create','--session',original,'--file','checkpoint.txt')[0]['checkpoint']
    (root/'checkpoint.txt').write_text('after checkpoint\n')
    checkpoint_after=cli('checkpoint','create','--session',original,'--file','checkpoint.txt')[0]['checkpoint']
    comparison=cli('eval','compare',checkpoint,checkpoint_after)[0]
    check('checkpoint comparison uses changed file',any(row['path']=='checkpoint.txt' for row in comparison['files_changed']))
    terminal=Terminal(['rollback',checkpoint]);terminal.read_until('Type yes to restore')
    (root/'checkpoint.txt').write_text('changed while reviewing\n')
    terminal.send('yes\n');terminal.finish(code=1)
    check('restore refuses concurrent human edit',(root/'checkpoint.txt').read_text()=='changed while reviewing\n')
    terminal=Terminal(['rollback',checkpoint]);terminal.read_until('Type yes to restore');terminal.send('yes\n');terminal.finish()
    check('checkpoint restored exact selected file',(root/'checkpoint.txt').read_text()=='before checkpoint\n')
    check('checkpoint preserved unrelated file',(root/'unrelated.txt').read_text()=='leave this alone\n')
    cli('checkpoint','list')
    cli('eval','define','check-file','--file','checkpoint.txt','--command','test -f checkpoint.txt')
    terminal=Terminal(['eval','run','check-file']);terminal.approve_next();terminal.read_until('"passed": true');terminal.finish()
    cli('eval','define','check-failure','--file','checkpoint.txt','--command','printf actual-check-failure >&2; exit 7')
    terminal=Terminal(['eval','run','check-failure']);terminal.approve_next();terminal.read_until('actual-check-failure');terminal.finish(code=1)
    cli('tool','disable','bash')
    denied=cli('eval','run','check-file',expected=1)
    check('saved check obeys disabled shell',not any(row.get('passed') is True for row in denied))
    cli('tool','enable','bash')
    cli('memory','remember','fruit','apricot')
    facts=cli('memory','list')[0]['memory']
    check('saved memory is visible after process restart',any(row['key']=='fruit' and row['content']=='apricot' for row in facts))
    cli('skill','capture','remember-fruit',original)
    cli('skill','run','remember-fruit')
    teams=cli('swarm','Read the project and reply without tools.')
    result=teams[-1]['team']
    check('swarm creates actual separate saved sessions',result['status']=='completed' and len({m['session_id'] for m in result['members']})==4)
    member=result['members'][0]['id']
    cli('creature','inspect',member)
    cli('pack','incident-response','--objective','Read the project and reply without tools.')
    job=cli('schedule','once:'+time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime(time.time()+2)),'scheduled greeting')[0]['job']
    cli('up')
    cli('daemon','status')
    until=time.monotonic()+25
    while time.monotonic()<until:
        info=cli('jobs','inspect',job)[0]
        if info['handle']['status']=='completed': break
        time.sleep(.2)
    check('scheduled actual model request completes once',info['handle']['status']=='completed' and info['handle']['run_count']==1)
    cli('down')
    time.sleep(.5)
    cli('up')
    time.sleep(.5)
    check('service restart does not repeat completed task',cli('jobs','inspect',job)[0]['handle']['run_count']==1)
    watch=cli('watch','read changed files','--pattern','watched/**')[0]['job']
    cli('jobs','pause',watch);cli('jobs','resume',watch);cli('jobs','cancel',watch)
    check('watcher cancellation is saved',cli('jobs','inspect',watch)[0]['handle']['status']=='cancelled')
    cli('down')
    world=cli('world','spawn','local_shell')[0]['id']
    check('world listed after restart',any(w['id']==world for w in cli('world','list')[0]['worlds']))
    terminal=Terminal(['world','exec',world,'printf world-command-ok'])
    terminal.approve_next();terminal.read_until('world-command-ok');terminal.finish()
    cli('world','pause',world)
    cli('run','unused','--world',world,expected=1)
    cli('world','resume',world)
    with requests_lock: before=len(requests)
    running=subprocess.Popen([str(binary),'--format','json','run','stall','--world',world],cwd=root,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    owned_processes.append(running)
    until=time.monotonic()+5
    while time.monotonic()<until:
        with requests_lock: sent=any(row['messages'][-1]['content']=='stall' for row in requests[before:])
        if sent: break
        time.sleep(.01)
    check('world model request really started',sent)
    stopped=time.monotonic();cli('world','close',world)
    stdout,stderr=running.communicate(timeout=2)
    check('world close cancels waiting provider',running.returncode!=0 and time.monotonic()-stopped<2,stdout+stderr)
    cli('run','unused','--world',world,expected=1)

    fixture=root/'tool-server.py'
    fixture.write_text('''import sys,json
from pathlib import Path
for line in sys.stdin:
 request=json.loads(line)
 if 'id' not in request: continue
 method=request['method']
 if method=='initialize': result={'protocolVersion':'2024-11-05','capabilities':{'tools':{},'resources':{}},'serverInfo':{'name':'fixture','version':'1'}}
 elif method=='tools/list': result={'tools':[{'name':'read','description':'Read the controlled fixture','inputSchema':{'type':'object','properties':{}}}]}
 elif method=='tools/call':
  Path('external-result.txt').write_text('actual external call')
  result={'content':[{'type':'text','text':'actual external call'}],'isError':False}
 elif method=='resources/list': result={'resources':[{'uri':'test://fixture','name':'fixture'}]}
 elif method=='resources/read': result={'contents':[{'uri':'test://fixture','mimeType':'text/plain','text':'actual resource'}]}
 else: result={}
 print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}),flush=True)
''')
    servers=root/'servers.json'
    servers.write_text(json.dumps([{'name':'fixture','transport':{'type':'stdio'},'command':sys.executable,'args':[str(fixture)],'env':{'FIXTURE_SECRET':'MCP_SECRET_MUST_NOT_DISPLAY'}}]))
    for operation,expected in ((['tools','fixture'],'read'),(['resources','fixture'],'test://fixture'),(['read','fixture','test://fixture'],'actual resource'),(['healthcheck','fixture'],'true'),(['call','fixture','read'],'actual external call')):
        terminal=Terminal(['mcp','--config',str(servers),*operation]);terminal.approve_next();terminal.read_until(expected);terminal.finish()
    terminal=Terminal(['run','external read','--mcp-config',str(servers)])
    terminal.read_until('Type yes or no:')
    check('startup display hides configured secret',b'MCP_SECRET_MUST_NOT_DISPLAY' not in terminal.output)
    terminal.approve_next();terminal.approve_next();terminal.read_until('Answer saved to session');terminal.finish()
    check('agent used the actual external process',(root/'external-result.txt').read_text()=='actual external call')
    cli('notify','destination','local-inbox','--terminal')
    terminal=Terminal(['notify','send','local-inbox','Test subject','Test message'])
    terminal.read_until('Type yes or no:');terminal.send('yes\n');terminal.finish()
    inbox=cli('notify','inbox')[0]['inbox']
    check('local message delivered without invented acknowledgement',len(inbox)==1 and inbox[0]['receipt']['acknowledged'] is False)
    cli('notify','acknowledge',inbox[0]['id'])
    check('explicit acknowledgement is saved',cli('notify','inspect',inbox[0]['id'])[0]['receipt']['acknowledged'] is True)
except BaseException as error:
    if not isinstance(error,SystemExit) or error.code not in (None,0):
        failure=f'{type(error).__name__}: {error}'
    raise
finally:
    # Persist the check failure before cleanup so a cleanup failure cannot hide
    # the original evidence or leave the run without a report.
    save_report()
    terminals={terminal.proc.pid:terminal for terminal in owned_terminals}
    for process in owned_processes:
        cleanup_process(process,terminals.get(process.pid))
    # Do not launch an unrelated cleanup command in a terminal-only run.
    # Stop only a service that this disposable project's test actually started.
    if service_started:
        try:
            with (root/'service-cleanup.txt').open('wb') as log:
                process=subprocess.Popen([str(binary),'--format','json','down'],cwd=root,env=env,stdout=log,stderr=log)
                if poll_until_exit(process,15) is None:cleanup_process(process)
                elif process.returncode!=0:cleanup.append({'service':'down','status':'failed','exit_code':process.returncode})
        except Exception as error:cleanup.append({'service':'down','status':'failed','error':str(error)})
        save_report()
    # HTTPServer.shutdown waits for its serving thread. Bound that wait too;
    # both server threads are daemon threads and cannot hold this driver open.
    closer=threading.Thread(target=server.shutdown,daemon=True)
    closer.start();closer.join(2)
    if closer.is_alive():cleanup.append({'server':'shutdown','status':'failed','error':'server shutdown exceeded two seconds'})
    else:server.server_close()
    save_report()
    cleanup_failed=any(row['status']=='failed' for row in cleanup)
    print(json.dumps({'workspace':str(root),'checks':len(results),'passed':sum(row['pass'] for row in results),'failed':[row for row in results if not row['pass']],'failure':failure,'cleanup_failed':cleanup_failed}),flush=True)
    if cleanup_failed and failure is None:sys.exit(1)
