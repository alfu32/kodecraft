"""Synthetic deterministic transport peer; no database/user settings are read."""
import json, os, struct, sys, threading, time, uuid, subprocess
mode, pid_path = sys.argv[1:3]
with open(pid_path, 'w') as file: file.write(str(os.getpid()))
if mode == 'descendant':
    descendant = subprocess.Popen([sys.executable, '-c', 'import time;time.sleep(60)'])
    with open(pid_path + '.child', 'w') as file: file.write(str(descendant.pid))
write_lock = threading.Lock()
finished = threading.Event()
epoch = str(uuid.uuid4())
def send(value, fragmented=False):
    data = json.dumps(value, ensure_ascii=False).encode()
    frame = struct.pack('>I',len(data)) + data
    with write_lock:
        if fragmented:
            for byte in frame:
                sys.stdout.buffer.write(bytes([byte])); sys.stdout.buffer.flush()
        else:
            sys.stdout.buffer.write(frame); sys.stdout.buffer.flush()
def result(id, value): send({'jsonrpc':'2.0','id':id,'result':value})
def perform(message):
    id, method, params = message['id'], message['method'], message.get('params',{})
    if method == 'test.echo':
        time.sleep(params.get('delay',0)); result(id,params)
    elif method == 'test.block':
        finished.wait(30); result(id,{'outcome':'cancelled'})
    elif method == 'query.cancel':
        finished.set(); result(id,{'status':'requested'})
    elif method == 'test.crash':
        sys.stderr.write('synthetic-password-marker' * 10000);sys.stderr.flush();os._exit(7)
    elif method == 'test.flood':
        for sequence in range(40): send({'jsonrpc':'2.0','method':'query.event','params':{'sequence':sequence,'value':'x'}})
        result(id,{'done':True})
    elif method == 'test.error':
        send({'jsonrpc':'2.0','id':id,'error':{'code':-32603,'message':'synthetic-password-marker','data':{'sqlState':'42000','vendorCode':123,'message':'synthetic-password-marker'}}})
    elif method == 'test.notifications':
        for count in range(100): send({'jsonrpc':'2.0','method':'driver.progress','params':{'resolutionId':'synthetic-driver','phase':'download','completedBytes':count,'totalBytes':100}})
        for count in range(3): send({'jsonrpc':'2.0','method':'query.event','params':{'executionId':'synthetic-query','sequence':count}})
        result(id,{'done':True})
    elif method == 'driver.resolve':
        resolution=params['resolutionId']
        send({'jsonrpc':'2.0','method':'driver.progress','params':{'resolutionId':resolution,'phase':'download','completedBytes':8192,'totalBytes':-1}})
        if params['driver']['id']=='synthetic-block':
            finished.wait(20)
            send({'jsonrpc':'2.0','id':id,'error':{'code':-32800,'message':'Driver resolution cancelled','data':{'notStarted':True}}})
        else:
            result(id,{'providerId':'synthetic','resolvedVersion':'1.0','classpathDigest':'0'*64,'source':'local-classpath','manifestPath':'/synthetic/install-v1.json','classpath':['/synthetic/driver.jar'],'reused':False})
    elif method == 'driver.cancel':
        finished.set();result(id,{'status':'requested'})
    else: send({'jsonrpc':'2.0','id':id,'error':{'code':-32601,'message':'Unknown method'}})
while True:
    header=sys.stdin.buffer.read(4)
    if not header: break
    if len(header)!=4: sys.exit(2)
    length=struct.unpack('>I',header)[0];data=sys.stdin.buffer.read(length)
    message=json.loads(data);method=message['method'];id=message['id']
    if method=='hello':
        if mode in ('timeout','descendant'): time.sleep(60);continue
        if mode in ('poison','zero','oversize','truncated'):
            raw={'poison':b'driver-stdout','zero':b'\0\0\0\0','oversize':struct.pack('>I',8388609),'truncated':b'\0\0\0\x08{'}[mode]
            sys.stdout.buffer.write(raw);sys.stdout.buffer.flush();sys.exit(2)
        send({'jsonrpc':'2.0','id':id,'result':{'protocolVersion':99 if mode=='wrong-version' else 1,'helperVersion':'fixture','javaVersion':'21.0.9','helperEpoch':epoch,'maxFrameBytes':8388608,'capabilities':['hello','ping','shutdown','query.cancel']}},True)
        if mode=='slow-reader': time.sleep(0.5)
    elif method=='ping': result(id,{'status':'ok','helperEpoch':epoch})
    elif method=='shutdown': result(id,{'closing':True});break
    else:
        with open(pid_path+'.requests','a') as file: file.write(method+'\n')
        threading.Thread(target=perform,args=(message,),daemon=True).start()
