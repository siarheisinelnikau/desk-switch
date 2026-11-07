#!/usr/bin/env python3
"""Synthetic microphone integration check; run under dbus-run-session.

Requires PipeWire utilities, WirePlumber 0.5+ (policy-only profile), and a debug
build. Uses a private socket/daemon, no hardware monitors and temporary profiles.
"""
import os,pathlib,subprocess,tempfile,time,json,math,array,select,signal
ROOT=pathlib.Path(__file__).resolve().parents[1]
work=pathlib.Path(tempfile.mkdtemp(prefix='desk-switch-mic-qa-'))
(work/'runtime').mkdir(mode=0o700)
for name in ['config','data','state']: (work/name).mkdir()
env=dict(os.environ,XDG_RUNTIME_DIR=str(work/'runtime'),PIPEWIRE_RUNTIME_DIR=str(work/'runtime'),PIPEWIRE_REMOTE='desk-switch-test',XDG_CONFIG_HOME=str(work/'config'),XDG_DATA_HOME=str(work/'data'),XDG_STATE_HOME=str(work/'state'),GVFS_DISABLE_FUSE='1',GIO_USE_VFS='local')
# A private daemon with only protocol, in-memory nodes and adapters. No ALSA,
# Bluetooth, camera, portal or other hardware monitor is loaded.
config=work/'pipewire.conf'
config.write_text('''context.properties = { core.daemon = true core.name = desk-switch-test default.clock.rate = 48000 default.clock.quantum = 480 default.clock.min-quantum = 480 }
context.spa-libs = { support.* = support/libspa-support audio.convert.* = audioconvert/libspa-audioconvert audiotestsrc = audiotestsrc/libspa-audiotestsrc }
context.modules = [ { name = libpipewire-module-protocol-native } { name = libpipewire-module-access args = { access.socket = { desk-switch-test = unrestricted } } } { name = libpipewire-module-metadata } { name = libpipewire-module-spa-node-factory } { name = libpipewire-module-client-node } { name = libpipewire-module-client-device } { name = libpipewire-module-adapter } { name = libpipewire-module-link-factory } { name = libpipewire-module-session-manager } ]
context.objects = [ { factory = spa-node-factory args = { factory.name = support.node.driver node.name = Dummy-Driver node.group = pipewire.dummy priority.driver = 20000 } } { factory = adapter args = { factory.name = audiotestsrc node.name = fixture-mic node.description = "Synthetic microphone" node.virtual = false media.class = Audio/Source audio.position = [ FL FR ] node.param.Props = { live = true wave = sine freq = 1000 volume = 0.2 } } } ]
''')
log=(work/'run.log').open('w');procs=[]
def start(args,**kwargs):
 p=subprocess.Popen(args,env=kwargs.pop('env',env),stdout=kwargs.pop('stdout',log),stderr=log,**kwargs);procs.append(p);return p
def run(args,**kwargs):return subprocess.check_output(args,env=env,text=True,timeout=5,**kwargs)
def graph():return json.loads(run(['pw-dump']))
def nodes():return [o for o in graph() if o['type']=='PipeWire:Interface:Node']
def until(fn,seconds=6):
 end=time.monotonic()+seconds
 while time.monotonic()<end:
  result=fn()
  if result:return result
  time.sleep(.05)
 raise AssertionError('timeout; '+(work/'run.log').read_text()[-5000:])
try:
 daemon=start(['pipewire','-c',str(config)])
 until(lambda:(work/'runtime/desk-switch-test').exists())
 # The installed policy-only profile has no hardware monitoring components.
 policy=start(['wireplumber','--profile','policy'])
 until(lambda:any(o['info']['props'].get('node.name')=='fixture-mic' for o in nodes()))
 binary=str(ROOT/'target/debug/desk-switch')
 a=work/'a';b=work/'b'
 def appenv(profile):return dict(env,DESK_SWITCH_STATE=str(profile),DESK_SWITCH_MANAGED='1')
 def appcmd(profile,*args):return subprocess.check_output([binary,*args],env=appenv(profile),text=True,timeout=5).strip()
 ka=appcmd(a,'init');kb=appcmd(b,'init');appcmd(a,'pair',kb);appcmd(b,'pair',ka)
 port=47879
 sender=start([binary,'microphone','serve',f'127.0.0.1:{port}','--source','fixture-mic'],env=appenv(a),stdin=subprocess.PIPE)
 time.sleep(.15)
 receiver=start([binary,'microphone','play',f'127.0.0.1:{port}','--volume','100'],env=appenv(b),stdin=subprocess.PIPE)
 until(lambda:any(o['info']['props'].get('node.name')=='desk-switch-microphone' for o in nodes()))
 probe=start(['pw-record','--raw','--format=s16','--rate=48000','--channels=2','--target=desk-switch-microphone','-'],stdout=subprocess.PIPE,stdin=subprocess.DEVNULL)
 def samples(seconds=1):
  data=bytearray();end=time.monotonic()+seconds
  while time.monotonic()<end:
   if select.select([probe.stdout],[],[],.1)[0]:data.extend(os.read(probe.stdout.fileno(),65536))
  a=array.array('h');a.frombytes(data[:len(data)//2*2]);return a
 first=samples(1.5);assert first and max(map(abs,first))>1000,('no transmitted microphone tone',first[:10],(work/'run.log').read_text())
 # After startup, a 1 kHz tone must not disappear in whole 10 ms blocks.
 # This catches transport reconnect loops and playback starvation that a
 # nonzero peak or average gain check alone would miss.
 steady=first[len(first)//3:];blocks=[steady[i:i+960] for i in range(0,len(steady)-959,960)]
 missing=sum(max(map(abs,block))<100 for block in blocks)
 assert blocks and missing<=max(1,len(blocks)//50),('audio dropouts',missing,len(blocks),(work/'run.log').read_text()[-5000:])
 print(f'PASS continuous synthetic audio: {len(blocks)} blocks, {missing} silent dropouts',flush=True)
 receiver.stdin.write(b'volume 25\n');receiver.stdin.flush();samples(.4)
 quarter=samples(1);assert quarter
 def rms(a):return math.sqrt(sum(x*x for x in a)/len(a))
 ratio=rms(quarter)/rms(first)
 assert .15 < ratio < .38,('wrong microphone gain',ratio)
 receiver.stdin.write(b'stop\n');receiver.stdin.flush();assert receiver.wait(timeout=5)==0
 until(lambda:not any(o['info']['props'].get('node.name','').startswith('desk-switch-microphone') for o in nodes()))
 assert sender.poll() is None
 receiver2=start([binary,'microphone','play',f'127.0.0.1:{port}'],env=appenv(b),stdin=subprocess.PIPE)
 until(lambda:any(o['info']['props'].get('node.name')=='desk-switch-microphone' for o in nodes()))
 time.sleep(.2)
 receiver2.kill();receiver2.wait(timeout=3)
 until(lambda:not any(o['info']['props'].get('node.name','').startswith('desk-switch-microphone') for o in nodes()))
 print('PASS: microphone PCM, 25% live gain, stop and force-kill both close capture/virtual input; private PipeWire with synthetic tone only',flush=True)
 print('QA directory:',work,flush=True)
finally:
 for p in reversed(procs):
  if p.poll() is None:
   try:p.terminate();p.wait(timeout=5)
   except subprocess.TimeoutExpired:p.kill();p.wait(timeout=2)
 log.close()
