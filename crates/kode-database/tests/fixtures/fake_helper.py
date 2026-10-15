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
connections = {}
attempts = {}
cancelled_attempts = set()
connection_lock = threading.Lock()
cancelled_queries = set()
query_waits = {}
query_modes = {}
def connection_info(params):
    return {'connectionId':str(uuid.uuid4()),'dataSourceId':params['dataSourceId'],'capabilities':{'transactions':True,'savepoints':True,'catalogs':True,'schemas':True},'productName':'Synthetic','productVersion':'1','driverName':'Fixture','driverVersion':'1','autoCommit':params.get('autoCommit',True),'readOnly':params.get('readOnly',False),'catalog':params.get('catalog'),'schema':params.get('schema'),'contextRevision':1,'stateRevision':1,'transactionUsable':True}
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
        execution=params.get('executionId','')
        if query_modes.get(execution)=='fixture:unsupported': result(id,{'status':'unsupported'});return
        cancelled_queries.add(execution)
        if execution in query_waits: query_waits[execution].set()
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
    elif method == 'driver.inspect':
        result(id,{'providerId':'synthetic','classpathDigest':'application-classpath','loaderId':'fixture-loader','driverClasses':['fixture.Driver'],'selectedDriverClass':'fixture.Driver','driverMajorVersion':1,'driverMinorVersion':0,'source':'application-classpath','classpath':[]})
    elif method in ('connection.open','connection.test'):
        attempt=params['attemptId']
        if 'blocked' in params['jdbcUrl']:
            deadline=time.monotonic()+20
            while attempt not in cancelled_attempts and time.monotonic()<deadline: time.sleep(0.005)
        with connection_lock:
            if attempt in cancelled_attempts:
                send({'jsonrpc':'2.0','id':id,'error':{'code':-32021,'message':'Opening cancelled','data':{'notStarted':True}}});return
            state=connection_info(params)
            if 'credentials' in params['jdbcUrl'] and (params['properties'].get('password')!='synthetic-password' or params['properties'].get('token')!='synthetic-token' or params['properties'].get('user')!='synthetic-user'):
                send({'jsonrpc':'2.0','id':id,'error':{'code':-32020,'message':'Operation failed','data':{'sqlError':{'message':'Synthetic authentication failure','sqlState':'28000','vendorCode':42,'chained':[]}}}});return
            if method=='connection.test':
                state.pop('connectionId');state['success']=True;state['latencyMs']=1
            else:
                connections[state['connectionId']]=(state,params);attempts[attempt]=state['connectionId']
            result(id,state)
    elif method == 'connection.close':
        with connection_lock:
            cancelled_attempts.add(params['attemptId']);connection=attempts.pop(params['attemptId'],None)
            if connection: connections.pop(connection,None)
            finished.set();result(id,{'closed':True})
    elif method in ('connection.setContext','connection.commit','connection.rollback'):
        with connection_lock:
            pair=connections.get(params['connectionId'])
            if pair is None:
                send({'jsonrpc':'2.0','id':id,'error':{'code':-32021,'message':'Stale connection','data':{'notStarted':True}}});return
            state,opening=pair
        if opening['properties'].get('fixtureMode')=='blockCommit' and method=='connection.commit': finished.wait(20)
        with connection_lock:
            for key in ('catalog','schema','autoCommit','readOnly'):
                if key in params: state[key]=params[key]
            state['contextRevision']+=1;state['stateRevision']+=1;result(id,state)
    elif method == 'query.execute':
        execution=params['executionId'];sql=params['sql'];wait=threading.Event();query_waits[execution]=wait;query_modes[execution]=sql
        with connection_lock:
            pair=connections.get(params['connectionId'])
        if pair is None:
            send({'jsonrpc':'2.0','id':id,'error':{'code':-32021,'message':'Stale connection','data':{'notStarted':True}}});return
        state=pair[0]
        if execution in cancelled_queries:
            result(id,{'executionId':execution,'outcome':'cancelled','elapsedMs':1,'transactionUsable':True,'previewIncomplete':True,'eventCount':0,'connectionState':state});return
        def event(sequence,value): send({'jsonrpc':'2.0','method':'query.event','params':{'executionId':execution,'sequence':sequence,'event':value}})
        event(0,{'type':'started'});event(1,{'type':'resultSetStart','resultSetId':execution+'-r','columns':[{'label':'value','jdbcType':12,'typeName':'VARCHAR','nullable':True}]})
        event(9 if sql=='fixture:bad-sequence' else 2,{'type':'rows','resultSetId':execution+'-r','rows':[[{'kind':'text','text':'synthetic-private-row'}],[{'kind':'null'}],[{'kind':'text','text':''}]]})
        if sql in ('fixture:block','fixture:crash-pending','fixture:unsupported'): wait.wait(20)
        cancelled=execution in cancelled_queries
        if not cancelled: event(3,{'type':'resultSetEnd','resultSetId':execution+'-r','limited':False,'incomplete':False})
        state=dict(state);state['stateRevision']+=1
        result(id,{'executionId':execution,'outcome':'cancelled' if cancelled else 'completed','elapsedMs':1,'transactionUsable':True,'previewIncomplete':cancelled,'eventCount':3 if cancelled else 4,'connectionState':state})
        if sql=='fixture:final-exit': sys.stdout.buffer.flush();os._exit(0)
    elif method == 'test.release-queries':
        for wait in query_waits.values(): wait.set()
        result(id,{'released':True})
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
        send({'jsonrpc':'2.0','id':id,'result':{'protocolVersion':99 if mode=='wrong-version' else 1,'helperVersion':'fixture','javaVersion':'21.0.9','helperEpoch':epoch,'maxFrameBytes':8388608,'capabilities':['hello','ping','shutdown','query.cancel','query.execute']}},True)
        if mode=='slow-reader': time.sleep(0.5)
    elif method=='ping': result(id,{'status':'ok','helperEpoch':epoch})
    elif method=='shutdown': result(id,{'closing':True});break
    else:
        with open(pid_path+'.requests','a') as file: file.write(method+'\n')
        threading.Thread(target=perform,args=(message,),daemon=True).start()
