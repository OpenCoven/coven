"""Portable token wire and owned temporary publication controls; no native API."""
import copy
import base64
import subprocess
import tempfile
import hashlib
import json
import os
import uuid
from unittest import mock
from pathlib import Path
import re
import traceback
import unittest
import token_receipt as module

COMPONENT_SOURCE=Path(__file__).with_name('token_result.py').read_bytes()

def component():
    return {'kind':'coven.effective-token-result.v1','captureAttempted':False,'privilegeName':'SeSystemProfilePrivilege','result':{'failure':0,'nativeError':0,'cleanupFailed':False,'clearAttempts':6,'clearFailures':0,'freeAttempts':6,'freeFailures':0,'closeAttempts':2,'closeFailures':0,'facts':{'context':1,'tokenType':1,'impersonationLevel':None,'elevationType':3,'elevated':False,'endpointsMatch':True,'privilege':{'present':False,'enabled':False,'enabledByDefault':False,'removed':False,'usedForAccess':False}}}}

def source():
    files=[{'id':key,'sha256':hashlib.sha256(COMPONENT_SOURCE if key=='result-validator' else key.encode()).hexdigest()} for key in sorted(module.SOURCE_PATHS)]
    return {'commit':'a'*40,'files':files,'beforeAfterMatch':True}

def host():
    return {'kind':'workflow-pwsh','assemblyScope':'selected-direct-managed-assemblies','executableSha256':'b'*64,'assemblies':[{'kind':key,'sha256':'c'*64,'version':[7,6,6,0] if key=='powershell' else [10,0,0,0]} for key in module.ASSEMBLIES],'nativeLibraries':[{'kind':key,'sha256':'d'*64} for key in module.LIBRARIES],'beforeAfterMatch':True}

def probe():return {'kind':'coven.effective-token-probe.v1','captureAttempted':False,'source':source(),'host':host(),'component':json.dumps(component(),separators=(',',':'))}
def encoded(value):return json.dumps(value,separators=(',',':')).encode()

class ProbeWireTests(unittest.TestCase):
    def decode(self,value,expected_source=None,expected_host=None,decoder=COMPONENT_SOURCE):
        return module.decode_probe(encoded(value),source() if expected_source is None else expected_source,host() if expected_host is None else expected_host,decoder)
    def reject(self,change):
        value=probe();change(value)
        with self.assertRaisesRegex(ValueError,'^Invalid bounded token receipt$'):self.decode(value)
    def test_valid_source_host_component_exit(self):
        wire,result,code=self.decode(probe());self.assertEqual(wire,probe());self.assertEqual(result,component());self.assertEqual(code,0)
    def test_all_primary_and_cleanup_failure_exits(self):
        for failure in range(1,11):
            c=component();c['result'].update(failure=failure,facts=None);p=probe();p['component']=json.dumps(c);self.assertEqual(self.decode(p)[2],1)
        c=component();c['result'].update(cleanupFailed=True,clearFailures=1,facts=None);p=probe();p['component']=json.dumps(c);self.assertEqual(self.decode(p)[2],1)
        c={'kind':'coven.effective-token-export-failure.v1','captureAttempted':False,'factsWithheld':True,'exportError':'output-limit','resultSummary':{k:v for k,v in component()['result'].items() if k!='facts'}};p=probe();p['component']=json.dumps(c);self.assertEqual(self.decode(p)[2],2)
    def test_source_inventory_matches_independent_producer(self):
        text=Path(__file__).with_name('token-binding.ps1').read_text().split('function Read-CovenTokenSourceBytes')[0]
        entries=dict(re.findall(r"'([a-z-]+)'='([^']+)'",text));self.assertEqual(module.SOURCE_PATHS,entries);self.assertEqual(len(entries),30)
    def test_closed_objects(self):
        for path in [(),('source',),('source','files',0),('host',),('host','assemblies',0),('host','nativeLibraries',0)]:
            def change(value):
                for part in path:value=value[part]
                value['privatePath']='PRIVATE_SENTINEL'
            with self.subTest(path=path):self.reject(change)
    def test_source_fixed_ids_order_complete_and_hashes(self):
        self.reject(lambda p:p['source']['files'].pop());self.reject(lambda p:p['source']['files'].reverse())
        self.reject(lambda p:p['source']['files'][0].update(id=p['source']['files'][1]['id']))
        for key,bad in [('commit','A'*40),('beforeAfterMatch',1),('beforeAfterMatch',False)]:self.reject(lambda p:p['source'].__setitem__(key,bad))
        for bad in ['A'*64,'a'*63,True,None]:self.reject(lambda p:p['source']['files'][0].__setitem__('sha256',bad))
    def test_host_closed_kinds_order_versions_hashes(self):
        for key,bad in [('kind','PRIVATE_SENTINEL'),('assemblyScope','all-assemblies'),('beforeAfterMatch',1),('beforeAfterMatch',False),('executableSha256','invalid')]:self.reject(lambda p:p['host'].__setitem__(key,bad))
        self.reject(lambda p:p['host']['assemblies'].reverse());self.reject(lambda p:p['host']['nativeLibraries'].reverse())
        self.reject(lambda p:p['host']['assemblies'].pop());self.reject(lambda p:p['host']['nativeLibraries'].pop())
        for bad in [True,-1,2**32,'7',None]:self.reject(lambda p:p['host']['assemblies'][0]['version'].__setitem__(0,bad))
        self.reject(lambda p:next(a for a in p['host']['assemblies'] if a['kind']=='powershell')['version'].__setitem__(0,8))
    def test_both_expected_identity_sides_validated(self):
        for expected in ['source','host']:
            value=probe();other=copy.deepcopy(value[expected]);other['beforeAfterMatch']=1
            value[expected]['beforeAfterMatch']=1
            with self.assertRaises(ValueError):self.decode(value,**{'expected_'+expected:other})
        other=source();other['files'][0]['sha256']='f'*64
        with self.assertRaises(ValueError):self.decode(probe(),expected_source=other)
        other=host();other['executableSha256']='f'*64
        with self.assertRaises(ValueError):self.decode(probe(),expected_host=other)
    def test_matching_malformed_identity_cannot_bypass_validation(self):
        changes=[('source',lambda d:d['files'].reverse()),('source',lambda d:d['files'][0].update(id='PRIVATE_SENTINEL')),
                 ('host',lambda d:d['assemblies'].reverse()),('host',lambda d:d['nativeLibraries'].reverse()),
                 ('host',lambda d:d['assemblies'][0]['version'].__setitem__(0,True)),
                 ('host',lambda d:d['nativeLibraries'][0].update(sha256='invalid'))]
        for key,change in changes:
            value=probe();change(value[key]);expected=copy.deepcopy(value[key])
            with self.subTest(key=key),self.assertRaises(ValueError):self.decode(value,**{'expected_'+key:expected})

    def test_component_string_and_independent_closed_result(self):
        for bad in [component(),None,True,'x'*4097,'{"kind":"PRIVATE_SENTINEL"}']:
            self.reject(lambda p:p.__setitem__('component',bad))
        c=component();c['result']['facts']['privatePath']='PRIVATE_SENTINEL';self.reject(lambda p:p.__setitem__('component',json.dumps(c)))
        c=component();c['result']['facts']['elevated']=1;self.reject(lambda p:p.__setitem__('component',json.dumps(c)))
    def test_captured_decoder_hash_and_size(self):
        for bad in [COMPONENT_SOURCE+b'\n',b'',b'x'*262145,'not-bytes']:
            with self.assertRaises(ValueError):self.decode(probe(),decoder=bad)
    def test_encoding_duplicates_constants_depth_and_outer_cap(self):
        valid=encoded(probe())
        for bad in [valid+b' '*32768,b'\xffPRIVATE_SENTINEL',b'{',b'['*10000,b'{"kind":NaN}',b'{"kind":"one","kind":"two"}']:
            with self.assertRaises(ValueError):module.decode_probe(bad,source(),host(),COMPONENT_SOURCE)
        duplicate=valid.replace(b'{"kind":',b'{"captureAttempted":false,"kind":',1)
        with self.assertRaises(ValueError):module.decode_probe(duplicate,source(),host(),COMPONENT_SOURCE)
        for key in ['kind','captureAttempted']:
            self.reject(lambda p:p.__setitem__(key,1))
    def test_actual_csharp_probe_wire(self):
        # The real encoder is exercised with typed synthetic facts. Never call
        # CovenOwnedTokenCollector or any token/adapter native entry here.
        with tempfile.TemporaryDirectory(prefix='coven-token-wire-controlled-') as owned:
            root=Path(owned)
            names=['token-privileges.cs','token-reader.cs','token-native.cs','token-public.cs','token-binding-types.cs']
            for name in names+['token-binding.ps1','query-helper.ps1','query-summary.ps1']:(root/name).write_bytes(Path(__file__).with_name(name).read_bytes())
            controlled_source=envelope_fixture()[0]['source']
            (root/'identities.json').write_text(json.dumps({'source':controlled_source,'host':host(),'phases':envelope_fixture()[0]['query']['phases']}))
            script=r'''$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'token-binding.ps1')
. (Join-Path $PSScriptRoot 'query-helper.ps1')
. (Join-Path $PSScriptRoot 'query-summary.ps1')
$parts=foreach($name in @('token-privileges.cs','token-reader.cs','token-native.cs','token-public.cs','token-binding-types.cs')){"namespace CovenTokenBound {`n"+[IO.File]::ReadAllText((Join-Path $PSScriptRoot $name))+"`n}"}
$parts+=@'
namespace CovenTokenBound {public static class Fixture {public static CovenTokenQueryResult Create(int failure,int cleanup){return new CovenTokenQueryResult((CovenTokenFailure)failure,failure==4?5u:0u,6,cleanup,6,0,2,0,new CovenEffectiveTokenFacts(CovenTokenContext.Process,1,null,3,false,new CovenTokenPrivilegeFacts(true,1)));}}}
'@
$refs=@(Get-ChildItem (Join-Path $PSHOME 'ref') -Filter '*.dll' | ForEach-Object FullName)
Add-Type -TypeDefinition ($parts -join "`n") -ReferencedAssemblies $refs
$data=Get-Content -Raw (Join-Path $PSScriptRoot 'identities.json') | ConvertFrom-Json
$files=[Collections.Generic.List[CovenTokenBound.CovenTokenSourceFileIdentity]]::new()
foreach($file in $data.source.files){$files.Add([CovenTokenBound.CovenTokenSourceFileIdentity]::new($file.id,$file.sha256))}
$source=[CovenTokenBound.CovenTokenSourceIdentity]::new($data.source.commit,$files.ToArray(),$true)
$assemblies=[Collections.Generic.List[CovenTokenBound.CovenTokenAssemblyIdentity]]::new()
foreach($file in $data.host.assemblies){$assemblies.Add([CovenTokenBound.CovenTokenAssemblyIdentity]::new($file.kind,$file.sha256,[uint[]]$file.version))}
$libraries=[Collections.Generic.List[CovenTokenBound.CovenTokenLibraryIdentity]]::new()
foreach($file in $data.host.nativeLibraries){$libraries.Add([CovenTokenBound.CovenTokenLibraryIdentity]::new($file.kind,$file.sha256))}
$hostProof=[CovenTokenBound.CovenTokenHostIdentity]::new($data.host.executableSha256,$assemblies.ToArray(),$libraries.ToArray(),$true)
$rows=foreach($case in @(@(0,0),@(4,0),@(0,1),@(4,1))){
 $result=[CovenTokenBound.Fixture]::Create($case[0],$case[1])
 $encoded=[CovenTokenBound.CovenTokenBindingPublic]::EncodeProbe($result,$source,$hostProof)
 $query=[CovenQueryResult]::new()
 $query.Started=$true;$query.RootExited=$true;$query.ReadersSettled=$true;$query.Reaped=$true
 $query.ExitCode=$encoded.ExitCode;$query.ElapsedMs=42
 $query.Stdout=[Text.UTF8Encoding]::new($false,$true).GetString($encoded.Payload)
 foreach($property in $data.phases.PSObject.Properties){if($property.Name -notin @('unit','frequency') -and $null -ne $property.Value){$query.Phases.($property.Name)=[long]$property.Value}}
 $packet=New-CovenTokenPrivatePacket -Source $source -HostProof $hostProof -Query $query
 $wrong=[ordered]@{source=$source;host=$hostProof;query=(ConvertTo-CovenTokenQuerySummary $query);probe=$query.Stdout} | ConvertTo-Json -Depth 16 -Compress
 [ordered]@{wire=[Convert]::ToBase64String($encoded.Payload);packet=[Convert]::ToBase64String($packet);uncanonicalPacket=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($wrong));exitCode=$encoded.ExitCode;failure=$case[0];cleanup=$case[1]}
}
$packetControls=0
function Reject-PacketControlled([scriptblock]$Action){
 $escaped=$null
 try {$escaped=& $Action;throw ('Controlled invalid packet admitted case='+$script:packetCase)}
 catch {if($_.Exception.Message.StartsWith('Controlled invalid packet admitted') -or $null -ne $escaped){throw}}
}
$originalProbe=$query.Stdout
foreach($bad in @('',('x'*32769),([string]::new([char]0,32768)),([string][char]233))){
 $script:packetCase='probe-'+$packetControls;$query.Stdout=$bad
 Reject-PacketControlled {New-CovenTokenPrivatePacket -Source $source -HostProof $hostProof -Query $query};$packetControls++
}
$script:packetCase='stderr';$query.Stdout=$originalProbe;$query.Stderr='x'*32769
Reject-PacketControlled {New-CovenTokenPrivatePacket -Source $source -HostProof $hostProof -Query $query};$packetControls++
$query.Stderr='';$script:packetCase='source-type'
Reject-PacketControlled {New-CovenTokenPrivatePacket -Source ([pscustomobject]@{Path='PRIVATE_SENTINEL'}) -HostProof $hostProof -Query $query};$packetControls++
$script:packetCase='query-type'
Reject-PacketControlled {New-CovenTokenPrivatePacket -Source $source -HostProof $hostProof -Query ([pscustomobject]@{Stdout=$originalProbe})};$packetControls++
[ordered]@{packetControls=$packetControls;rows=@($rows)} | ConvertTo-Json -Depth 5 -Compress
'''
            path=root/'encode.ps1';path.write_text(script)
            result=subprocess.run(['pwsh','-NoProfile','-File',str(path)],capture_output=True,timeout=180)
            self.assertEqual(result.returncode,0,result.stderr.decode());report=json.loads(result.stdout);self.assertEqual(report['packetControls'],7);rows=report['rows'];self.assertEqual(len(rows),4)
            for row in rows:
                data=base64.b64decode(row['wire'],validate=True)
                wire,value,code=module.decode_probe(data,controlled_source,host(),COMPONENT_SOURCE)
                envelope,packet_code=module.decode_packet(base64.b64decode(row['packet'],validate=True),controlled_source,host(),COMPONENT_SOURCE,PHASE_SOURCE)
                self.assertEqual(packet_code,code);self.assertEqual(envelope['component'],value)
                with self.assertRaises(ValueError):module.decode_packet(base64.b64decode(row['uncanonicalPacket'],validate=True),controlled_source,host(),COMPONENT_SOURCE,PHASE_SOURCE)
                self.assertEqual(code,row['exitCode']);self.assertEqual(value['result']['failure'],row['failure'])
                self.assertEqual(value['result']['cleanupFailed'],bool(row['cleanup']))
                self.assertEqual(value['result']['facts'] is None,bool(row['failure'] or row['cleanup']))

    def test_fixed_failure_never_exposes_decoder_or_wire(self):
        try:module.decode_probe(b'\xffPRIVATE_SENTINEL',source(),host(),COMPONENT_SOURCE)
        except ValueError:
            text=traceback.format_exc();self.assertNotIn('UnicodeDecodeError',text);self.assertNotIn('JSONDecodeError',text)
        else:self.fail('invalid input admitted')

PHASE_SOURCE=Path(__file__).with_name('public_receipt.py').read_bytes()

def envelope_fixture():
    src=source()
    for file in src['files']:
        if file['id']=='phase-validator':file['sha256']=hashlib.sha256(PHASE_SOURCE).hexdigest()
    wire=probe();wire['source']=src
    raw=encoded(wire)
    phases={'unit':'stopwatch-elapsed-ticks','frequency':1000,**{name:(None if name=='timeoutObservedTicks' else ordinal) for ordinal,name in enumerate(module.PHASE_NAMES)}}
    query={'kind':'effective-token-facts','stdoutDigestScope':'decoded-text-utf8',**dict.fromkeys(['started','rootExited','readersSettled','reaped'],True),**dict.fromkeys(['timedOut','outputLimited','hasFailure','hasCleanupFailure','hasReaderFailure'],False),'exitCode':0,'elapsedMs':42,'phases':phases,'stdoutChars':len(raw),'stderrChars':0,'stdoutSha256':hashlib.sha256(raw).hexdigest(),'stderrSha256':hashlib.sha256(b'').hexdigest()}
    value={'kind':'coven.effective-token-facts.v1','captureAttempted':False,'metadataComplete':False,'observationScope':'owned-child-effective-token','source':src,'host':host(),'query':query,'component':component()}
    return value,raw

class EnvelopeTests(unittest.TestCase):
    def validate(self,value,raw,phase=PHASE_SOURCE):return module.validate_envelope(value,value['source'],host(),raw,COMPONENT_SOURCE,phase)
    def reject(self,change):
        value,raw=envelope_fixture();change(value)
        with self.assertRaisesRegex(ValueError,'^Invalid bounded token receipt$'):self.validate(value,raw)
    def test_valid_envelope_and_bounded_encoding(self):
        value,raw=envelope_fixture();self.assertEqual(self.validate(value,raw),value)
        data=module.encode_bounded_json(value);self.assertLessEqual(len(data),65536);self.assertEqual(module.decode_envelope(data,value['source'],host(),raw,COMPONENT_SOURCE,PHASE_SOURCE),value)
    def test_failed_component_exit_and_cleanup_survive_whole_receipt(self):
        failure=component();failure['result'].update(failure=4,nativeError=5,facts=None)
        cleanup=component();cleanup['result'].update(cleanupFailed=True,closeFailures=1,facts=None)
        fallback={'kind':'coven.effective-token-export-failure.v1','captureAttempted':False,'factsWithheld':True,'exportError':'serialization-failed','resultSummary':{k:v for k,v in failure['result'].items() if k!='facts'}}
        for result,code in [(failure,1),(cleanup,1),(fallback,2)]:
            value,raw=envelope_fixture();wire=json.loads(raw);wire['component']=json.dumps(result);raw=encoded(wire)
            value['component']=result;value['query'].update(exitCode=code,stdoutChars=len(raw),stdoutSha256=hashlib.sha256(raw).hexdigest())
            self.validate(value,raw)
            value['query']['exitCode']=0
            with self.assertRaises(ValueError):self.validate(value,raw)

    def test_closed_scope_flags_and_extra_keys(self):
        for key,bad in [('kind','PRIVATE_SENTINEL'),('captureAttempted',0),('metadataComplete',True),('metadataComplete',0),('observationScope','future-controller')]:self.reject(lambda d:d.__setitem__(key,bad))
        self.reject(lambda d:d.__setitem__('privatePath','PRIVATE_SENTINEL'))
        self.reject(lambda d:d['query'].__setitem__('rawStderr','PRIVATE_SENTINEL'))
    def test_query_required_states_and_exit(self):
        for key in ['started','rootExited','readersSettled','reaped']:
            for bad in [False,1]:self.reject(lambda d:d['query'].__setitem__(key,bad))
        for key in ['timedOut','outputLimited','hasFailure','hasCleanupFailure','hasReaderFailure']:
            for bad in [True,0]:self.reject(lambda d:d['query'].__setitem__(key,bad))
        for bad in [True,1,2,3,None]:self.reject(lambda d:d['query'].__setitem__('exitCode',bad))
    def test_stdout_stderr_accounting_and_no_private_text(self):
        for key,bad in [('stdoutChars',0),('stdoutChars',True),('stdoutSha256','0'*64),('stderrChars',1),('stderrSha256','0'*64),('elapsedMs',True),('kind','kernel-file-metadata'),('stdoutDigestScope','raw-bytes')]:self.reject(lambda d:d['query'].__setitem__(key,bad))
    def test_phase_types_order_and_consistency(self):
        for key,bad in [('frequency',0),('frequency',True),('unit','wall-clock'),('cleanupFinishedTicks',-1),('rootWaitReturnedTicks',True),('startCallEnteredTicks',None),('timeoutObservedTicks',10)]:self.reject(lambda d:d['query']['phases'].__setitem__(key,bad))
        self.reject(lambda d:d['query']['phases'].__setitem__('cleanupFinishedTicks',0))
        self.reject(lambda d:d['query']['phases'].__setitem__('PRIVATE_SENTINEL',0))
    def test_public_component_validated_independently(self):
        self.reject(lambda d:d['component']['result']['facts'].__setitem__('elevated',0))
        self.reject(lambda d:d['component']['result']['facts'].__setitem__('tokenType',True))
        self.reject(lambda d:d['component']['result'].__setitem__('nativeError',False))
        self.reject(lambda d:d['component']['result']['facts']['privilege'].__setitem__('enabled',True))
    def test_captured_phase_validator_required(self):
        value,raw=envelope_fixture()
        for bad in [PHASE_SOURCE+b'\n',b'',b'x'*262145]:
            with self.assertRaises(ValueError):self.validate(value,raw,bad)
    def test_whole_wire_cap_duplicate_keys_and_private_encoding_failure(self):
        value,raw=envelope_fixture();data=module.encode_bounded_json(value)
        for bad in [data+b' '*65536,data.replace(b'{',b'{"captureAttempted":false,',1),b'\xffPRIVATE_SENTINEL']:
            with self.assertRaises(ValueError):module.decode_envelope(bad,value['source'],host(),raw,COMPONENT_SOURCE,PHASE_SOURCE)
        with self.assertRaises(ValueError):module.encode_bounded_json({'text':'x'*65536})
        with self.assertRaises(ValueError):module.encode_bounded_json({'x':float('nan')})

class PacketTests(unittest.TestCase):
    def packet(self):
        value,raw=envelope_fixture()
        return {'source':value['source'],'host':value['host'],'query':value['query'],'probe':raw.decode('ascii')}
    def decode(self,packet):return module.decode_packet(encoded(packet),self.packet()['source'],host(),COMPONENT_SOURCE,PHASE_SOURCE)
    def test_valid_packet_derives_only_whole_receipt(self):
        packet=self.packet();value,code=self.decode(packet);self.assertEqual(value,envelope_fixture()[0]);self.assertEqual(code,0)
        self.assertNotIn('probe',value)
    def test_packet_closed_keys_and_canonical_identity(self):
        for key in ['Source','Host','Query','Probe','privatePath']:
            packet=self.packet();packet[key]='PRIVATE_SENTINEL'
            with self.assertRaises(ValueError):self.decode(packet)
        packet=self.packet();packet['source']={key[0].upper()+key[1:]:value for key,value in packet['source'].items()}
        with self.assertRaises(ValueError):self.decode(packet)
    def test_packet_duplicate_valid_json_cap_and_encoding(self):
        packet=self.packet();data=encoded(packet)
        for raw in [data+b' '*131072,data.replace(b'{',b'{"probe":"",',1),b'\xffPRIVATE_SENTINEL']:
            with self.assertRaisesRegex(ValueError,'^Invalid bounded token receipt$'):module.decode_packet(raw,packet['source'],packet['host'],COMPONENT_SOURCE,PHASE_SOURCE)
    def test_probe_limits_private_shape_and_actual_query_mismatch(self):
        for bad in [None,True,{},'x'*32769,'é',self.packet()['probe']+' ']:
            packet=self.packet();packet['probe']=bad
            with self.assertRaises(ValueError):self.decode(packet)
        packet=self.packet();packet['query']['stderrChars']=1
        with self.assertRaises(ValueError):self.decode(packet)
    def test_parent_identity_mismatch(self):
        packet=self.packet();packet['source']['files'][0]['sha256']='f'*64
        with self.assertRaises(ValueError):self.decode(packet)
        packet=self.packet();packet['host']['executableSha256']='f'*64
        with self.assertRaises(ValueError):self.decode(packet)
    def test_failed_component_packet_preserves_exit(self):
        packet=self.packet();wire=json.loads(packet['probe']);result=component();result['result'].update(failure=4,nativeError=5,facts=None);wire['component']=json.dumps(result)
        packet['probe']=encoded(wire).decode();packet['query'].update(exitCode=1,stdoutChars=len(packet['probe']),stdoutSha256=hashlib.sha256(packet['probe'].encode()).hexdigest())
        value,code=self.decode(packet);self.assertEqual(code,1);self.assertEqual(value['component'],result)

class PublicationControls(unittest.TestCase):
 def setUp(self):
  self.owned=tempfile.TemporaryDirectory(prefix='coven-token-publication-controlled-');self.root=Path(self.owned.name);self.source_root=self.root/'source';self.source_root.mkdir()
  self.temp=self.root/'temporary';self.output=self.temp/('windows-reliability-token-'+uuid.uuid4().hex)/'public';self.output.mkdir(parents=True)
  self.value,self.raw=envelope_fixture()
  for key,relative in module.SOURCE_PATHS.items():
   data=COMPONENT_SOURCE if key=='result-validator' else PHASE_SOURCE if key=='phase-validator' else key.encode()
   path=self.source_root/relative;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(data)
   self.assertEqual(hashlib.sha256(data).hexdigest(),next(f['sha256'] for f in self.value['source']['files'] if f['id']==key))
  def git(*args):return subprocess.check_output(['git','-C',str(self.source_root),'-c','core.hooksPath=/dev/null','-c','commit.gpgsign=false',*args],stderr=subprocess.PIPE,text=True).strip()
  git('init','-q');git('add','.');git('-c','user.name=Controlled Fixture','-c','user.email=fixture@example.invalid','commit','-qm','controlled source fixture');commit=git('rev-parse','HEAD')
  self.value['source']['commit']=commit;wire=json.loads(self.raw);wire['source']=self.value['source'];self.raw=encoded(wire);self.value['query'].update(stdoutChars=len(self.raw),stdoutSha256=hashlib.sha256(self.raw).hexdigest())
 def tearDown(self):self.owned.cleanup()
 def publish(self):return module.publish_envelope(self.value,self.value['source'],host(),self.raw,COMPONENT_SOURCE,PHASE_SOURCE,self.output,self.temp,self.source_root)
 def reject(self,cleanup=False):
  with self.assertRaisesRegex(ValueError,'^Token receipt publication failed$') as caught:self.publish()
  self.assertEqual(caught.exception.cleanup_failed,cleanup)
 def test_success_object_hash_complete_and_no_stage(self):
  result=self.publish();path=self.output/'receipt.json';data=path.read_bytes();self.assertEqual(result,{'sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)});self.assertEqual(path.stat().st_nlink,1);self.assertLessEqual(len(data),65536)
  self.assertEqual(module.decode_envelope(data,self.value['source'],host(),self.raw,COMPONENT_SOURCE,PHASE_SOURCE),self.value);self.assertEqual([p.name for p in self.output.iterdir()],['receipt.json'])
 def test_existing_target_is_preserved(self):
  path=self.output/'receipt.json';path.write_bytes(b'foreign');self.reject();self.assertEqual(path.read_bytes(),b'foreign')
 def test_dangling_target_symlink_is_preserved(self):
  path=self.output/'receipt.json';path.symlink_to(self.root/'absent');self.reject();self.assertTrue(path.is_symlink())
 def test_output_directory_alias_refused(self):
  saved=self.output.with_name('saved');self.output.rename(saved);self.output.symlink_to(saved,target_is_directory=True);self.reject();self.assertEqual(list(saved.iterdir()),[])
 def test_source_root_alias_refused_before_resolution(self):
  saved=self.source_root.with_name('saved-source');self.source_root.rename(saved);self.source_root.symlink_to(saved,target_is_directory=True);self.reject();self.assertFalse((self.output/'receipt.json').exists())
 def test_hardlinked_source_same_bytes_clean_git_refused(self):
  source=self.source_root/module.SOURCE_PATHS['binding'];os.link(source,self.root/'owned-extra-link')
  state=subprocess.check_output(['git','-C',str(self.source_root),'status','--porcelain=v1']);self.assertEqual(state,b'');self.reject()
 def test_initial_stage_identity_failure_reports_cleanup_uncertainty(self):
  # Arm only after source reads, at the stage's first identity query.
  verify=module._verify_publication_source
  def arm(*args):
   verify(*args)
   if not hasattr(arm,'armed'):
    arm.armed=True;self.stage_fstat=mock.patch('os.fstat',side_effect=OSError('PRIVATE_SENTINEL'));self.stage_fstat.start()
  try:
   with mock.patch.object(module,'_verify_publication_source',side_effect=arm):self.reject(cleanup=True)
  finally:
   if hasattr(self,'stage_fstat'):self.stage_fstat.stop()
  files=list(self.output.iterdir());self.assertEqual(len(files),1);self.assertTrue(files[0].name.startswith('.receipt-'));self.assertFalse((self.output/'receipt.json').exists())
 def test_fsync_failure_removes_only_owned_stage(self):
  with mock.patch('os.fsync',side_effect=OSError('PRIVATE_SENTINEL')):self.reject()
  self.assertEqual(list(self.output.iterdir()),[])
 def test_atomic_link_collision_preserves_foreign_target(self):
  link=os.link
  def collision(src,dst,**kwargs):Path(dst).write_bytes(b'foreign');return link(src,dst,**kwargs)
  with mock.patch('os.link',side_effect=collision):self.reject()
  self.assertEqual((self.output/'receipt.json').read_bytes(),b'foreign');self.assertEqual(len(list(self.output.iterdir())),1)
 def test_replaced_target_preserved_after_identity_mismatch(self):
  link=os.link
  def replace(src,dst,**kwargs):link(src,dst,**kwargs);os.unlink(dst);Path(dst).write_bytes(b'foreign')
  with mock.patch('os.link',side_effect=replace):self.reject()
  self.assertEqual((self.output/'receipt.json').read_bytes(),b'foreign')
 def test_extra_hardlink_rejected_and_only_owned_public_paths_removed(self):
  link=os.link
  def extra(src,dst,**kwargs):link(src,dst,**kwargs);link(dst,self.root/'extra')
  with mock.patch('os.link',side_effect=extra):self.reject()
  self.assertEqual(list(self.output.iterdir()),[]);self.assertTrue((self.root/'extra').exists())
 def test_source_mutation_after_link_rejects_and_rolls_back(self):
  link=os.link
  def mutate(src,dst,**kwargs):link(src,dst,**kwargs);(self.source_root/module.SOURCE_PATHS['binding']).write_bytes(b'changed source')
  with mock.patch('os.link',side_effect=mutate):self.reject()
  self.assertEqual(list(self.output.iterdir()),[])
 def test_same_length_public_mutation_refused(self):
  link=os.link
  def mutate(src,dst,**kwargs):link(src,dst,**kwargs);data=Path(dst).read_bytes();Path(dst).write_bytes(b'['+data[1:])
  with mock.patch('os.link',side_effect=mutate):self.reject()
  self.assertEqual(list(self.output.iterdir()),[])
 def test_independent_redecode_failure_rolls_back(self):
  def refuse(*args):self.assertTrue((self.output/'receipt.json').exists());raise ValueError('PRIVATE_SENTINEL')
  with mock.patch.object(module,'decode_envelope',side_effect=refuse):self.reject()
  self.assertEqual(list(self.output.iterdir()),[])
 def test_secondary_stage_cleanup_failure_is_retained(self):
  unlink=os.unlink
  def fail_stage(path,*args,**kwargs):
   if Path(path).name.startswith('.receipt-'):raise PermissionError('PRIVATE_SENTINEL')
   return unlink(path,*args,**kwargs)
  with mock.patch('os.unlink',side_effect=fail_stage):self.reject(cleanup=True)
  self.assertFalse((self.output/'receipt.json').exists());self.assertEqual(len(list(self.output.iterdir())),1)


class FinalizerControls(unittest.TestCase):
 setUp=PublicationControls.setUp
 tearDown=PublicationControls.tearDown
 def bundle(self):
  private=self.output.parent/'private';private.mkdir(exist_ok=True);path=private/'bundle.json'
  path.write_bytes(encoded({'source':self.value['source'],'host':host(),'query':self.value['query'],'probe':self.raw.decode('ascii')}));return path
 def finish(self,path):
  executed=next(f['sha256'] for f in self.value['source']['files'] if f['id']=='validator')
  return module.finalize_token_bundle(path,self.temp,self.output,self.source_root,executed)
 def test_finalizer_removes_private_packet_before_publication(self):
  path=self.bundle();publish=module.publish_envelope
  def observed(*args):self.assertFalse(path.exists());return publish(*args)
  with mock.patch.object(module,'publish_envelope',side_effect=observed):result,code=self.finish(path)
  self.assertEqual(code,0);self.assertEqual(result['sha256'],hashlib.sha256((self.output/'receipt.json').read_bytes()).hexdigest())
 def test_finalizer_refuses_wrong_executed_validator(self):
  path=self.bundle()
  with self.assertRaises(module.TokenPublicationFailure):module.finalize_token_bundle(path,self.temp,self.output,self.source_root,'0'*64)
  self.assertFalse((self.output/'receipt.json').exists());self.assertFalse(path.exists())
 def test_bundle_replacement_preserved_and_cleanup_reported(self):
  path=self.bundle();decode=module.decode_packet
  def replace(*args):
   result=decode(*args);path.unlink();path.write_bytes(b'foreign');return result
  with mock.patch.object(module,'decode_packet',side_effect=replace):
   with self.assertRaises(module.TokenPublicationFailure) as caught:self.finish(path)
  self.assertTrue(caught.exception.cleanup_failed);self.assertEqual(path.read_bytes(),b'foreign');self.assertFalse((self.output/'receipt.json').exists())
 def test_bundle_unlink_failure_withholds_publication(self):
  path=self.bundle();unlink=os.unlink
  def fail(selected,*args,**kwargs):
   if Path(selected).resolve()==path.resolve():raise PermissionError('PRIVATE_SENTINEL')
   return unlink(selected,*args,**kwargs)
  with mock.patch('os.unlink',side_effect=fail):
   with self.assertRaises(module.TokenPublicationFailure) as caught:self.finish(path)
  self.assertTrue(caught.exception.cleanup_failed);self.assertTrue(path.exists());self.assertFalse((self.output/'receipt.json').exists())
 def test_bundle_wrong_job_output_refused(self):
  path=self.bundle();self.output=self.temp/'windows-reliability-token-different'/'public';self.output.mkdir(parents=True)
  with self.assertRaises(module.TokenPublicationFailure):self.finish(path)
  self.assertFalse((self.output/'receipt.json').exists())
 def test_bundle_alias_and_hardlink_refused(self):
  path=self.bundle();alias=path.with_name('alias');os.link(path,alias)
  with self.assertRaises(module.TokenPublicationFailure):self.finish(path)
  self.assertTrue(path.exists());alias.unlink();saved=path.with_name('saved');path.rename(saved);path.symlink_to(saved)
  with self.assertRaises(module.TokenPublicationFailure):self.finish(path)
  self.assertTrue(path.is_symlink());self.assertTrue(saved.exists())
 def test_private_directory_alias_refused(self):
  path=self.bundle();saved=path.parent.with_name('saved-private');path.parent.rename(saved);path.parent.symlink_to(saved,target_is_directory=True)
  with self.assertRaises(module.TokenPublicationFailure):self.finish(path)
  self.assertTrue((saved/'bundle.json').exists());self.assertFalse((self.output/'receipt.json').exists())
 def test_bundle_cap_duplicates_and_private_invalid_wire_refused(self):
  for bad in [b'x'*131073,b'{"probe":"a","probe":"b"}',b'\xffPRIVATE_SENTINEL']:
   path=self.bundle();path.write_bytes(bad)
   with self.assertRaisesRegex(module.TokenPublicationFailure,'^Token receipt publication failed$'):self.finish(path)
   self.assertFalse((self.output/'receipt.json').exists())
 def test_changed_source_or_git_refused(self):
  path=self.bundle();(self.source_root/'untracked').write_bytes(b'controlled')
  with self.assertRaises(module.TokenPublicationFailure):self.finish(path)
  self.assertFalse(path.exists());self.assertFalse((self.output/'receipt.json').exists())
 def test_failed_component_exit_survives_finalization(self):
  wire=json.loads(self.raw);result=component();result['result'].update(failure=4,nativeError=5,facts=None);wire['component']=json.dumps(result);self.raw=encoded(wire);self.value['query'].update(exitCode=1,stdoutChars=len(self.raw),stdoutSha256=hashlib.sha256(self.raw).hexdigest());self.value['component']=result
  path=self.bundle();result,code=self.finish(path);self.assertEqual(code,1);self.assertTrue((self.output/'receipt.json').exists());self.assertFalse(path.exists())

class PublisherCliControls(unittest.TestCase):
 def call(self,args=None,outcome=None,error=None):
  import io,contextlib
  output=io.StringIO();failure=io.StringIO()
  with mock.patch.object(module,'finalize_token_bundle',side_effect=error,return_value=outcome) as finalize,contextlib.redirect_stdout(output),contextlib.redirect_stderr(failure):
   code=module.run_token_publisher(['bundle','temporary','output','source'] if args is None else args,'a'*64)
  return code,output.getvalue(),failure.getvalue(),finalize
 def test_fixed_result_codes_and_exact_arguments(self):
  for code in (0,1,2):
   value={'sha256':'b'*64,'bytes':123};actual,text,error,called=self.call(outcome=(value,code));self.assertEqual(actual,code);self.assertEqual(json.loads(text),value);self.assertEqual(error,'');called.assert_called_once_with('bundle','temporary','output','source','a'*64)
 def test_fixed_failure_preserves_cleanup(self):
  for cleanup in (False,True):
   code,text,error,_=self.call(error=module.TokenPublicationFailure(cleanup));self.assertEqual((code,text,error),(1,'','token-publication-failed cleanupFailed='+str(cleanup).lower()+'\n'))
  code,text,error,_=self.call(error=RuntimeError('PRIVATE_SENTINEL'));self.assertEqual((code,text,error),(1,'','token-publication-failed cleanupFailed=false\n'))
 def test_bad_arguments_never_invoke_finalizer(self):
  for args in [[],['x']*5,[None]*4,'PRIVATE_SENTINEL']:
   code,text,error,called=self.call(args=args);self.assertEqual(code,1);self.assertEqual(text,'');called.assert_not_called();self.assertNotIn('PRIVATE',error)
 def test_invalid_public_result_is_withheld(self):
  for value,code in [({'sha256':'PRIVATE_SENTINEL','bytes':1},0),({'sha256':'b'*64,'bytes':True},0),({'sha256':'b'*64,'bytes':65537},0),({'sha256':'b'*64,'bytes':1},True),({'sha256':'b'*64,'bytes':1},3),({'sha256':'b'*64,'bytes':1,'private':'PRIVATE_SENTINEL'},0)]:
   actual,text,error,_=self.call(outcome=(value,code));self.assertEqual(actual,1);self.assertEqual(text,'');self.assertNotIn('PRIVATE',error)

class WholePublisherExecutionControls(unittest.TestCase):
 setUp=PublicationControls.setUp
 tearDown=PublicationControls.tearDown
 bundle=FinalizerControls.bundle
 def test_actual_captured_publisher_finalizes_synthetic_packet(self):
  scripts=Path(__file__).parent
  for key in ('validator','binding'):
   path=self.source_root/module.SOURCE_PATHS[key];path.write_bytes((scripts/Path(module.SOURCE_PATHS[key]).name).read_bytes())
  for item in self.value['source']['files']:item['sha256']=hashlib.sha256((self.source_root/module.SOURCE_PATHS[item['id']]).read_bytes()).hexdigest()
  def git(*args):return subprocess.check_output(['git','-C',str(self.source_root),'-c','core.hooksPath=/dev/null','-c','commit.gpgsign=false',*args],stderr=subprocess.PIPE,text=True).strip()
  git('add','.');git('-c','user.name=Controlled Fixture','-c','user.email=fixture@example.invalid','commit','-qm','controlled actual publisher')
  self.value['source']['commit']=git('rev-parse','HEAD');wire=json.loads(self.raw);wire['source']=self.value['source'];self.raw=encoded(wire);self.value['query'].update(stdoutChars=len(self.raw),stdoutSha256=hashlib.sha256(self.raw).hexdigest());bundle=self.bundle()
  build=self.root/'build.ps1';build.write_text(r'''param($Binder,$Directory,$Commit,$Python,$Bundle,$TemporaryRoot,$OutputDirectory,$Mode='publisher',$SnapshotHash='')
$ErrorActionPreference='Stop'
. $Binder
$capture=Get-CovenTokenSourceCapture $Directory $Commit
if($Mode -eq 'artifact'){
 New-CovenTokenArtifactInvocation -Capture $capture -Executable $Python -SourcePath $Bundle -SnapshotHash $SnapshotHash -TemporaryRoot $TemporaryRoot -OutputDirectory $OutputDirectory | ConvertTo-Json -Depth 4 -Compress
}else{
 New-CovenTokenPublisherInvocation -Capture $capture -Executable $Python -BundlePath $Bundle -TemporaryRoot $TemporaryRoot -OutputDirectory $OutputDirectory | ConvertTo-Json -Depth 4 -Compress
}
''')
  import sys
  built=subprocess.run(['pwsh','-NoProfile','-File',str(build),str(self.source_root/module.SOURCE_PATHS['binding']),str(self.source_root),self.value['source']['commit'],sys.executable,str(bundle),str(self.temp),str(self.output)],capture_output=True,timeout=120)
  self.assertEqual(built.returncode,0,built.stderr.decode());invocation=json.loads(built.stdout)
  expected=next(f['sha256'] for f in self.value['source']['files'] if f['id']=='validator');self.assertEqual(invocation['PublisherSha256'],expected)
  executed=subprocess.run([sys.executable,*invocation['Arguments']],capture_output=True,timeout=60)
  self.assertEqual(executed.returncode,0,executed.stderr.decode());self.assertEqual(executed.stderr,b'');result=json.loads(executed.stdout);data=(self.output/'receipt.json').read_bytes()
  self.assertEqual(result,{'sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)});self.assertFalse(bundle.exists());self.assertEqual(module.decode_envelope(data,self.value['source'],host(),self.raw,COMPONENT_SOURCE,PHASE_SOURCE),self.value)

  # A separate captured entry re-admits the exact public file from a bounded
  # baseline and closed parent outcome. All token/query data here are synthetic.
  response=executed.stdout;publisher=copy.deepcopy(self.value['query']);publisher.pop('kind');publisher.pop('stdoutDigestScope');publisher.update(stdoutChars=len(response),stdoutSha256=hashlib.sha256(response).hexdigest())
  proof={'kind':'coven.effective-token-execution.v1','captureAttempted':False,'metadataComplete':False,'sourceBound':True,'hostBound':True,'referencesReleased':True,'cleanupFailed':False,'failureStage':'none','query':self.value['query'],'publisherQuery':publisher,'publication':result,'publicationSucceeded':True,'exitCode':0}
  baseline=self.output.parent/'source.json';baseline.write_bytes(encoded(self.value['source']));snapshot=hashlib.sha256(baseline.read_bytes()).hexdigest()
  built=subprocess.run(['pwsh','-NoProfile','-File',str(build),str(self.source_root/module.SOURCE_PATHS['binding']),str(self.source_root),self.value['source']['commit'],sys.executable,str(baseline),str(self.temp),str(self.output),'artifact',snapshot],capture_output=True,timeout=120)
  self.assertEqual(built.returncode,0,built.stderr.decode());admission=json.loads(built.stdout);self.assertEqual(admission['PublisherSha256'],expected)
  env=os.environ.copy();env.update(EXECUTION_JSON=json.dumps(proof),PUBLIC_RECEIPT=str(self.output/'receipt.json'))
  admitted=subprocess.run([sys.executable,*admission['Arguments']],capture_output=True,env=env,timeout=60)
  self.assertEqual((admitted.returncode,admitted.stdout,admitted.stderr),(0,b'token-artifact-admission-pass\n',b''))
  admission['Arguments'][-1]='0'*64
  rejected=subprocess.run([sys.executable,*admission['Arguments']],capture_output=True,env=env,timeout=60)
  self.assertEqual((rejected.returncode,rejected.stdout,rejected.stderr),(1,b'',b'token-artifact-admission-failed\n'));self.assertEqual((self.output/'receipt.json').read_bytes(),data)

class ArtifactAdmissionControls(unittest.TestCase):
 setUp=PublicationControls.setUp
 tearDown=PublicationControls.tearDown
 publish=PublicationControls.publish
 def prepare(self):
  publication=self.publish();response=json.dumps(publication,sort_keys=True,separators=(',',':')).encode()+b'\n'
  publisher=copy.deepcopy(self.value['query']);publisher.pop('kind');publisher.pop('stdoutDigestScope');publisher.update(stdoutChars=len(response),stdoutSha256=hashlib.sha256(response).hexdigest())
  self.execution={'kind':'coven.effective-token-execution.v1','captureAttempted':False,'metadataComplete':False,'sourceBound':True,'hostBound':True,'referencesReleased':True,'cleanupFailed':False,'failureStage':'none','query':copy.deepcopy(self.value['query']),'publisherQuery':publisher,'publication':publication,'publicationSucceeded':True,'exitCode':0}
  return publication
 def admit(self):return module.admit_token_artifact(self.output,self.temp,self.source_root,self.value['source'],self.execution)
 def reject(self):
  with self.assertRaisesRegex(ValueError,'^Token artifact admission failed$'):self.admit()
 def rebind(self):
  path=self.output/'receipt.json';data=path.read_bytes();publication={'sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)};self.execution['publication']=publication;response=json.dumps(publication,sort_keys=True,separators=(',',':')).encode()+b'\n';self.execution['publisherQuery'].update(stdoutChars=len(response),stdoutSha256=hashlib.sha256(response).hexdigest())
 def rewrite(self,change):
  path=self.output/'receipt.json';value=json.loads(path.read_bytes());change(value);path.write_bytes(encoded(value));self.rebind()
 def test_admitted_exact_published_object_without_reconstructing_probe(self):
  expected=self.prepare();self.assertEqual(self.admit(),expected)
 def test_closed_parent_flags_and_boolean_integer_distinctions(self):
  self.prepare();original=copy.deepcopy(self.execution)
  for key,bad in [('kind','PRIVATE_SENTINEL'),('captureAttempted',0),('metadataComplete',0),('sourceBound',1),('hostBound',False),('referencesReleased',False),('cleanupFailed',True),('publicationSucceeded',1),('failureStage','query'),('exitCode',False)]:
   self.execution=copy.deepcopy(original);self.execution[key]=bad;self.reject()
  self.execution=copy.deepcopy(original);self.execution['private']='PRIVATE_SENTINEL';self.reject()
 def test_parent_publication_hash_size_and_output_accounting(self):
  self.prepare();original=copy.deepcopy(self.execution)
  for path,bad in [(('publication','sha256'),'0'*64),(('publication','bytes'),True),(('publication','bytes'),65537),(('publisherQuery','stdoutChars'),1),(('publisherQuery','stdoutSha256'),'0'*64)]:
   self.execution=copy.deepcopy(original);self.execution[path[0]][path[1]]=bad;self.reject()
 def test_parent_query_shapes_and_equality(self):
  self.prepare();original=copy.deepcopy(self.execution)
  for group,key,bad in [('query','started',1),('publisherQuery','started',1),('query','stdoutChars',True),('query','stdoutSha256','0'*64),('publisherQuery','stderrChars',True),('publisherQuery','reaped',False),('publisherQuery','timedOut',True)]:
   self.execution=copy.deepcopy(original);self.execution[group][key]=bad;self.reject()
 def test_public_closed_shape_and_component_checked_even_with_rebound_hash(self):
  self.prepare();path=self.output/'receipt.json';original=path.read_bytes()
  for change in [lambda value:value.__setitem__('privatePath','PRIVATE_SENTINEL'),lambda value:value.__setitem__('captureAttempted',0),lambda value:value['component']['result']['facts'].__setitem__('elevated',0),lambda value:value['query'].__setitem__('started',1),lambda value:value['query']['phases'].__setitem__('cleanupFinishedTicks',0)]:
   path.write_bytes(original);self.rewrite(change);self.reject()
 def test_public_hardlink_and_alias_refused_without_deletion(self):
  self.prepare();path=self.output/'receipt.json';extra=self.root/'extra';os.link(path,extra);self.reject();extra.unlink();saved=path.with_name('saved');path.rename(saved);path.symlink_to(saved);self.reject();self.assertTrue(path.is_symlink());self.assertTrue(saved.exists())
 def test_changed_public_bytes_without_matching_parent_refused(self):
  self.prepare();path=self.output/'receipt.json';data=path.read_bytes()
  value=json.loads(data);reordered=encoded(dict(reversed(list(value.items()))));self.assertEqual(len(reordered),len(data));self.assertEqual(json.loads(reordered),value);self.assertNotEqual(reordered,data)
  path.write_bytes(reordered);self.reject()
  path.write_bytes(b'['+data[1:]);self.reject();self.assertTrue(path.exists())
 def test_invalid_phase_order_with_matching_parent_query_refused(self):
  self.prepare();self.rewrite(lambda value:value['query']['phases'].__setitem__('cleanupFinishedTicks',0));self.execution['query']=copy.deepcopy(json.loads((self.output/'receipt.json').read_bytes())['query']);self.reject()
 def test_source_mutation_and_dirty_git_refused(self):
  self.prepare();(self.source_root/'untracked').write_bytes(b'controlled');self.reject();(self.source_root/'untracked').unlink();(self.source_root/module.SOURCE_PATHS['binding']).write_bytes(b'changed');self.reject()
 def test_publication_diagnostics_preserve_failed_component_exit(self):
  failure=component();failure['result'].update(failure=4,nativeError=5,facts=None);wire=json.loads(self.raw);wire['component']=json.dumps(failure);self.raw=encoded(wire);self.value['component']=failure;self.value['query'].update(exitCode=1,stdoutChars=len(self.raw),stdoutSha256=hashlib.sha256(self.raw).hexdigest());expected=self.prepare();self.execution['exitCode']=1;self.assertEqual(self.admit(),expected)
 def test_final_read_change_refused(self):
  self.prepare();read=module._read_stable_regular;path=self.output/'receipt.json';calls=0
  def change(selected,limit):
   nonlocal calls
   if selected.name=='receipt.json':
    calls+=1
    if calls==2:selected.write_bytes(b'changed final read')
   return read(selected,limit)
  with mock.patch.object(module,'_read_stable_regular',side_effect=change):self.reject()
 def artifact_entry(self,source_hash=None,snapshot_hash=None,execution=None,receipt_path=None):
  import io,contextlib
  baseline=encoded(self.value['source']);path=self.output.parent/'source.json';path.write_bytes(baseline)
  arguments=[str(path),str(self.temp),str(self.output),str(self.source_root),hashlib.sha256(baseline).hexdigest() if snapshot_hash is None else snapshot_hash]
  executed=next(file['sha256'] for file in self.value['source']['files'] if file['id']=='validator') if source_hash is None else source_hash
  output=io.StringIO();failure=io.StringIO()
  with mock.patch.dict(os.environ,{'EXECUTION_JSON':json.dumps(self.execution) if execution is None else execution,'PUBLIC_RECEIPT':str(self.output/'receipt.json') if receipt_path is None else receipt_path}),contextlib.redirect_stdout(output),contextlib.redirect_stderr(failure):code=module.run_token_artifact_admission(arguments,executed)
  return code,output.getvalue(),failure.getvalue()
 def test_fixed_artifact_entry_and_hash_bound_baseline(self):
  self.prepare();self.assertEqual(self.artifact_entry(),(0,'token-artifact-admission-pass\n',''))
  for changes in [{'source_hash':'0'*64},{'snapshot_hash':'0'*64},{'execution':'PRIVATE_SENTINEL'},{'receipt_path':str(self.root/'foreign.json')}]:self.assertEqual(self.artifact_entry(**changes),(1,'','token-artifact-admission-failed\n'))
 def test_artifact_entry_rejects_snapshot_hardlink_and_alias(self):
  self.prepare();self.artifact_entry();path=self.output.parent/'source.json';extra=self.root/'snapshot-extra';os.link(path,extra);self.assertEqual(self.artifact_entry(),(1,'','token-artifact-admission-failed\n'));extra.unlink();saved=path.with_name('snapshot-saved');path.rename(saved);path.symlink_to(saved);self.assertEqual(self.artifact_entry(),(1,'','token-artifact-admission-failed\n'));self.assertTrue(saved.exists())

if __name__=='__main__':unittest.main(verbosity=2)
