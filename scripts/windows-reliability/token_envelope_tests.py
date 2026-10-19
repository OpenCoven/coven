"""Synthetic binder/collector ordering only; no native token/host admission."""
from pathlib import Path
import base64, gzip, hashlib, json, os, re, subprocess, sys, tempfile, unittest
PROBE=Path(__file__).with_name('token-facts-probe.ps1')
def psq(s): return "'"+str(s).replace("'","''")+"'"
BINDER=r'''
$script:mode=MODE
$script:events=EVENTS
$script:sourceChecks=0
$script:repo=$null
function Event([string]$Value){[IO.File]::AppendAllText($script:events,$Value+"`n")}
function Open-CovenTokenRuntimeBinding {Event 'open';if($script:mode -eq 'open-fails'){throw 'PRIVATE_HOST_PATH'};[pscustomobject]@{Owned=$true}}
function Get-CovenTokenSourceCapture {param($Directory,$Commit);Event 'capture';$script:repo=$Directory;$hash=(Get-FileHash (Join-Path $Directory 'scripts/windows-reliability/token-binding.ps1')).Hash.ToLowerInvariant();$probeHash=(Get-FileHash (Join-Path $Directory 'scripts/windows-reliability/token-facts-probe.ps1')).Hash.ToLowerInvariant();if($script:mode -eq 'wrong-binder'){$hash='0'*64};if($script:mode -eq 'wrong-probe'){$probeHash='0'*64};[pscustomobject]@{Files=@([pscustomobject]@{Id='binding';Sha256=$hash},[pscustomobject]@{Id='probe';Sha256=$probeHash})}}
function Add-CovenTokenCapturedTypes {param($Capture);Event 'compile';Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Text;
namespace CovenTokenBound {
 public static class CovenOwnedTokenCollector {
  public static string Events="";
  public static string Mode="";
  public static object Collect(){File.AppendAllText(Events,"collect\n");if(Mode=="collect-fails"||Mode=="collect-and-close-fail")throw new Exception("PRIVATE_TOKEN_MARKER");if(Mode=="post-dirty")File.WriteAllText(Path.Combine(Path.GetDirectoryName(Events),"repo","dirty.txt"),"controlled");return new object();}
 }
 public sealed class Encoded {public byte[] Payload=Encoding.UTF8.GetBytes("controlled-payload");public int ExitCode=0;}
 public static class CovenTokenBindingPublic {public static Encoded EncodeProbe(object result,object source,object host){File.AppendAllText(CovenOwnedTokenCollector.Events,"encode\n");var value=new Encoded();if(CovenOwnedTokenCollector.Mode=="result-fails")value.ExitCode=1;if(CovenOwnedTokenCollector.Mode=="oversize")value.Payload=new byte[32769];return value;}}
}
'@;[CovenTokenBound.CovenOwnedTokenCollector]::Events=$script:events;[CovenTokenBound.CovenOwnedTokenCollector]::Mode=$script:mode}
function Test-CovenTokenSourceCapture {param($Capture);Event 'source-check';$script:sourceChecks++;return -not ($script:mode -eq 'source-changed' -and $script:sourceChecks -eq 2)}
function Test-CovenTokenRuntimeBinding {param($Binding);Event 'host-check';return $true}
function ConvertTo-CovenTokenSourceDto {param($Capture);Event 'source-dto';if($script:mode -eq 'post-head'){& git -C $script:repo update-ref HEAD HEAD^ | Out-Null;if($LASTEXITCODE -ne 0){throw 'controlled fixture'}};return [object]::new()}
function ConvertTo-CovenTokenHostDto {param($Binding);Event 'host-dto';return [object]::new()}
function Close-CovenTokenRuntimeBinding {param($Binding);Event 'close';if($script:mode -in @('close-fails','collect-and-close-fail')){throw 'PRIVATE_RELEASE_PATH'}}
'''
class ProbeControls(unittest.TestCase):
 def run_case(self, mode, *, dirty=False, wrong_head=False):
  with tempfile.TemporaryDirectory(prefix='coven-token-probe-controlled-') as owned:
   parent=Path(owned); root=parent/'repo';root.mkdir();events=parent/'events.txt'
   scripts=root/'scripts/windows-reliability';scripts.mkdir(parents=True)
   probe_path=scripts/PROBE.name;probe_path.write_bytes(PROBE.read_bytes());(scripts/'token-binding.ps1').write_text(BINDER.replace('MODE',psq(mode)).replace('EVENTS',psq(events)))
   def git(*args):return subprocess.check_output(['git','-C',str(root),'-c','core.hooksPath=/dev/null','-c','commit.gpgsign=false',*args],stderr=subprocess.PIPE,text=True).strip()
   git('init','-q');git('add','.');git('-c','user.name=Controlled Fixture','-c','user.email=fixture@example.invalid','commit','-qm','controlled fixture')
   git('-c','user.name=Controlled Fixture','-c','user.email=fixture@example.invalid','commit','--allow-empty','-qm','controlled second fixture')
   commit=git('rev-parse','HEAD')
   if dirty:(root/'dirty.txt').write_text('controlled')
   p=subprocess.run(['pwsh','-NoProfile','-File',str(probe_path),'-Directory',str(root),'-Commit','a'*40 if wrong_head else commit,'-ProbeSourceHash',hashlib.sha256(probe_path.read_bytes()).hexdigest()],capture_output=True,timeout=180)
   observed=events.read_text().splitlines() if events.exists() else []
   self.assertNotIn(b'PRIVATE',p.stdout+p.stderr)
   return p,observed
 def failure(self,mode,stage,cleanup=False,**kwargs):
  p,events=self.run_case(mode,**kwargs);self.assertEqual(p.returncode,2);self.assertEqual(p.stdout,b'');self.assertEqual(p.stderr,f'token-probe-failed stage={stage} cleanupFailed={str(cleanup).lower()}\n'.encode());return events
 def test_success_order(self):
  p,events=self.run_case('ok');self.assertEqual((p.returncode,p.stdout,p.stderr),(0,b'controlled-payload',b''));self.assertEqual(events,['open','capture','compile','source-check','host-check','collect','source-dto','host-dto','encode','source-check','host-check','close'])
 def test_component_failure_exit_preserved(self):
  p,e=self.run_case('result-fails');self.assertEqual((p.returncode,p.stdout,p.stderr),(1,b'controlled-payload',b''));self.assertEqual(e[-1],'close')
 def test_open_failure_has_no_owned_release(self):self.assertEqual(self.failure('open-fails','runtime-binding'),['open'])
 def test_wrong_binder_refuses_compilation(self):self.assertEqual(self.failure('wrong-binder','source-binding'),['open','capture','close'])
 def test_wrong_executed_probe_hash_refuses_compilation(self):self.assertEqual(self.failure('wrong-probe','source-binding'),['open','capture','close'])
 def test_wrong_head_refuses_compilation(self):self.assertEqual(self.failure('ok','source-binding',wrong_head=True),['open','capture','close'])
 def test_dirty_source_refuses_compilation(self):self.assertEqual(self.failure('ok','source-binding',dirty=True),['open','capture','close'])
 def test_collector_failure_releases(self):self.assertEqual(self.failure('collect-fails','collection')[-2:],['collect','close'])
 def test_release_failure_withholds_payload(self):self.assertEqual(self.failure('close-fails','reference-release',True)[-1],'close')
 def test_primary_stage_survives_release_failure(self):self.assertEqual(self.failure('collect-and-close-fail','collection',True)[-2:],['collect','close'])
 def test_postcollection_source_failure_withholds_payload(self):self.assertEqual(self.failure('source-changed','final-binding')[-2:],['source-check','close'])
 def test_postcollection_dirty_source_refuses_output(self):self.assertEqual(self.failure('post-dirty','final-binding')[-1],'close')
 def test_postcollection_head_change_refuses_output(self):self.assertEqual(self.failure('post-head','final-binding')[-1],'close')
 def test_probe_cap_refuses_output(self):self.assertEqual(self.failure('oversize','encoding')[-2:],['encode','close'])

class InvocationControls(unittest.TestCase):
 def test_captured_script_bytes_and_closed_arguments(self):
  script=r'''
param([Parameter(Mandatory)][string]$Binder)
$ErrorActionPreference='Stop'
. $Binder
$root=Join-Path ([IO.Path]::GetTempPath()) ("coven-token-invocation-'é-"+';$covenControlledSentinel=`literal-'+[Guid]::NewGuid().ToString('N'))
$other=$root+'-other'
$inventory=Get-CovenTokenSourceInventory
function Require([bool]$Value){if(-not $Value){throw 'Controlled invocation assertion failed'}}
function Reject([scriptblock]$Action){try{& $Action | Out-Null}catch{return};throw 'Invalid invocation admitted'}
$cases=0
[IO.Directory]::CreateDirectory($root) | Out-Null
try {
 foreach($relative in $inventory.Values){$path=Join-Path $root $relative;[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($path))|Out-Null;[IO.File]::WriteAllText($path,'controlled source')}
 $probePath=Join-Path $root $inventory['probe']
 $probe=@'
param([string]$Directory,[string]$Commit,[string]$ProbeSourceHash)
$directoryHash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($Directory))).ToLowerInvariant()
[Console]::Out.Write(([ordered]@{kind='controlled-probe-execution';directoryHash=$directoryHash;commit=$Commit;probeHash=$ProbeSourceHash}|ConvertTo-Json -Compress))
exit 0
'@
 [IO.File]::WriteAllText($probePath,$probe,[Text.UTF8Encoding]::new($false))
 $capture=Get-CovenTokenSourceCapture $root ('a'*40)
 $invocation=New-CovenTokenProbeInvocation $capture
 Require ($invocation.Arguments.Count -eq 5 -and ($invocation.Arguments[0..3] -join ',') -ceq '-NoLogo,-NoProfile,-NonInteractive,-EncodedCommand' -and $invocation.Arguments[4].Length -le 24576)
 # Substitute the owned disk path after construction. The captured bootstrap
 # must still execute the original bytes, never this replacement.
 [IO.File]::WriteAllText($probePath,'exit 97')
 try {$actual=& ([Environment]::ProcessPath) @($invocation.Arguments);Require ($LASTEXITCODE -eq 0)}
 finally {[IO.File]::WriteAllText($probePath,$probe,[Text.UTF8Encoding]::new($false))}
 $cases++
 $actual=$actual|ConvertFrom-Json -AsHashtable
 $expectedHash=Get-CovenTokenSourceHash ([IO.File]::ReadAllBytes($probePath))
 $rootHash=Get-CovenTokenSourceHash ([Text.Encoding]::UTF8.GetBytes($root))
 Require ($actual.kind -ceq 'controlled-probe-execution' -and $actual.commit -ceq ('a'*40) -and $actual.probeHash -ceq $expectedHash -and $actual.directoryHash -ceq $rootHash);$cases++
 foreach($quote in @([char]0x2018,[char]0x2019)){
  $quotedRoot=$root+[string]$quote
  [IO.Directory]::Move($root,$quotedRoot)
  try {
   $quotedCapture=Get-CovenTokenSourceCapture $quotedRoot ('a'*40)
   $quotedInvocation=New-CovenTokenProbeInvocation $quotedCapture
   $quotedActual=& ([Environment]::ProcessPath) @($quotedInvocation.Arguments)
   Require ($LASTEXITCODE -eq 0)
   $quotedActual=$quotedActual|ConvertFrom-Json -AsHashtable
   Require ($quotedActual.directoryHash -ceq (Get-CovenTokenSourceHash ([Text.Encoding]::UTF8.GetBytes($quotedRoot))) -and $quotedActual.commit -ceq ('a'*40) -and $quotedActual.probeHash -ceq $expectedHash)
  } finally {[IO.Directory]::Move($quotedRoot,$root)}
  $cases++
 }
 $entry=@($capture.Files|Where-Object Id -CEQ 'probe')[0]
 $oldPath=$entry.Path;$entry.Path=Join-Path $root 'wrong-probe.ps1'
 Reject {New-CovenTokenProbeInvocation $capture};$entry.Path=$oldPath;$cases++
 [IO.Directory]::CreateDirectory($other)|Out-Null
 $oldRoot=$capture.Directory;$capture.Directory=$other
 Reject {New-CovenTokenProbeInvocation $capture};$capture.Directory=$oldRoot;$cases++
 $entry.Bytes[0]=$entry.Bytes[0] -bxor 1
 Reject {New-CovenTokenProbeInvocation $capture};$entry.Bytes[0]=$entry.Bytes[0] -bxor 1;$cases++
 [IO.File]::WriteAllText($probePath,'changed disk source')
 Reject {New-CovenTokenProbeInvocation $capture};[IO.File]::WriteAllText($probePath,$probe,[Text.UTF8Encoding]::new($false));$cases++
 $commit=$capture.Commit;$capture.Commit='not-a-commit'
 Reject {New-CovenTokenProbeInvocation $capture};$capture.Commit=$commit;$cases++
 [IO.File]::WriteAllText($probePath,$probe+"`n#"+('x'*8192),[Text.UTF8Encoding]::new($false))
 $oversize=Get-CovenTokenSourceCapture $root ('a'*40)
 Reject {New-CovenTokenProbeInvocation $oversize};$cases++
} finally {if([IO.Directory]::Exists($root)){Remove-Item -LiteralPath $root -Recurse -Force};if([IO.Directory]::Exists($other)){[IO.Directory]::Delete($other)}}
"captured probe invocation PASS cases=$cases (owned synthetic probe only; no collector/native API)"
'''
  with tempfile.TemporaryDirectory(prefix='coven-token-invocation-harness-') as owned:
   path=Path(owned)/'controls.ps1';path.write_text(script)
   p=subprocess.run(['pwsh','-NoProfile','-File',str(path),'-Binder',str(PROBE.with_name('token-binding.ps1'))],capture_output=True,timeout=240)
   self.assertEqual(p.returncode,0,p.stderr.decode());self.assertIn(b'PASS cases=10',p.stdout)

class PublisherInvocationControls(unittest.TestCase):
 @classmethod
 def setUpClass(cls):
  cls.owned=tempfile.TemporaryDirectory(prefix='coven-publisher-invocation-');cls.base=Path(cls.owned.name);cls.root=cls.base/"repo‘’'é;$literal";cls.root.mkdir()
  cls.source=b"import json\ndef run_token_publisher(arguments,executed):\n print(json.dumps({'args':arguments,'executed':executed}));return 0\n"
  cls.binder=cls.base/'binder.ps1';cls.binder.write_bytes(PROBE.with_name('token-binding.ps1').read_bytes())
  cls.script=cls.base/'build.ps1';cls.script.write_text(r'''param($Binder,$Directory,$Python,$Mode)
$ErrorActionPreference='Stop'
. $Binder
$inventory=Get-CovenTokenSourceInventory
foreach($relative in $inventory.Values){$path=Join-Path $Directory $relative;[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($path))|Out-Null;if(-not [IO.File]::Exists($path)){[IO.File]::WriteAllText($path,'controlled source')}}
$capture=Get-CovenTokenSourceCapture $Directory ('a'*40)
$bundle=Join-Path $Directory 'private-packet.json'
if($Mode -eq 'relative'){$bundle='relative'}
if($Mode -eq 'changed'){$capture.Files[0].Bytes[0]=$capture.Files[0].Bytes[0] -bxor 1}
New-CovenTokenPublisherInvocation -Capture $capture -Executable $Python -BundlePath $bundle -TemporaryRoot $Directory -OutputDirectory (Join-Path $Directory 'public') | ConvertTo-Json -Depth 4 -Compress
''')
  cls.path=cls.root/'scripts/windows-reliability/token_receipt.py';cls.path.parent.mkdir(parents=True);cls.path.write_bytes(cls.source)
  result=cls.build();cls.invocation=json.loads(result.stdout);cls.bootstrap=cls.invocation['Arguments'][2]
  # The disk replacement cannot change already captured executable bytes.
  cls.path.write_bytes(b'raise SystemExit(97)')
 @classmethod
 def tearDownClass(cls):cls.owned.cleanup()
 @classmethod
 def build(cls,mode='normal'):
  p=subprocess.run(['pwsh','-NoProfile','-File',str(cls.script),str(cls.binder),str(cls.root),sys.executable,mode],capture_output=True,timeout=120)
  if mode=='normal' and p.returncode:raise AssertionError(p.stderr.decode())
  return p
 def execute(self,bootstrap=None):
  args=self.invocation['Arguments'].copy()
  if bootstrap is not None:args[2]=bootstrap
  return subprocess.run([sys.executable,*args],capture_output=True,timeout=30)
 def payload(self,compressed,source=None):
  script=re.sub(r"base64.b64decode\('[A-Za-z0-9+/=]+'", "base64.b64decode('"+base64.b64encode(compressed).decode()+"'",self.bootstrap)
  if source is not None:script=script.replace(hashlib.sha256(self.source).hexdigest(),hashlib.sha256(source).hexdigest())
  return script
 def reject(self,script):
  p=self.execute(script);self.assertEqual(p.returncode,1);self.assertEqual(p.stdout,b'');self.assertEqual(p.stderr,b'token-publication-failed cleanupFailed=false'+os.linesep.encode('ascii'))
 def test_exact_captured_bytes_and_paths_roundtrip(self):
  p=self.execute();self.assertEqual(p.returncode,0,p.stderr);value=json.loads(p.stdout);self.assertEqual(value['args'],self.invocation['Arguments'][3:]);self.assertEqual(value['executed'],hashlib.sha256(self.source).hexdigest());self.assertLessEqual(self.invocation['CommandCharacterBound'],32760)
 def test_wrong_hash_rejected_before_execution(self):self.reject(self.bootstrap.replace("hexdigest()=='"+hashlib.sha256(self.source).hexdigest(),"hexdigest()=='"+'0'*64))
 def test_truncated_compressed_stream_rejected(self):self.reject(self.payload(gzip.compress(self.source)[:-4]))
 def test_trailing_data_rejected(self):self.reject(self.payload(gzip.compress(self.source)+b'PRIVATE_SENTINEL'))
 def test_concatenated_member_rejected(self):self.reject(self.payload(gzip.compress(self.source)+gzip.compress(b'PRIVATE_SENTINEL')))
 def test_bounded_decompression_rejects_oversize(self):
  source=self.source+b'\n#'+b'x'*262144;self.reject(self.payload(gzip.compress(source),source))
 def test_invalid_compressed_data_fixed_failure(self):self.reject(self.payload(b'PRIVATE_SENTINEL'))
 def test_source_mutation_and_relative_argument_refused(self):
  for mode in ['changed','relative']:
   p=self.build(mode);self.assertNotEqual(p.returncode,0);self.assertIn(b'Token publisher invocation failed',p.stderr)
 def test_command_size_refused(self):
  self.path.write_bytes(b'#'+os.urandom(100000).hex().encode())
  p=self.build('large');self.assertNotEqual(p.returncode,0);self.assertIn(b'Token publisher invocation failed',p.stderr)

class RunnerInvocationControls(unittest.TestCase):
 @classmethod
 def setUpClass(cls):
  cls.owned=tempfile.TemporaryDirectory(prefix='coven-runner-invocation-');cls.base=Path(cls.owned.name).resolve();cls.root=cls.base/"repo‘’'é;$literal";cls.root.mkdir()
  cls.source=b"param($Directory,$Commit,$OutputDirectory,$TemporaryRoot,$RunnerSourceHash)\n[ordered]@{directory=$Directory;commit=$Commit;output=$OutputDirectory;temporary=$TemporaryRoot;runnerHash=$RunnerSourceHash}|ConvertTo-Json -Compress\nexit 0\n"
  cls.binder=cls.base/'binder.ps1';cls.binder.write_bytes(PROBE.with_name('token-binding.ps1').read_bytes())
  cls.script=cls.base/'build.ps1';cls.script.write_text(r"""param($Binder,$Directory,$Mode)
$ErrorActionPreference='Stop'
. $Binder
foreach($relative in (Get-CovenTokenSourceInventory).Values){$path=Join-Path $Directory $relative;[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($path))|Out-Null;if(-not [IO.File]::Exists($path)){[IO.File]::WriteAllText($path,'controlled source')}}
$capture=Get-CovenTokenSourceCapture $Directory ('a'*40)
$output=Join-Path $Directory 'public'
if($Mode -eq 'relative'){$output='relative'}
if($Mode -eq 'changed'){$capture.Files[0].Bytes[0]=$capture.Files[0].Bytes[0] -bxor 1}
New-CovenTokenRunnerInvocation -Capture $capture -OutputDirectory $output -TemporaryRoot $Directory -Executable ([Environment]::ProcessPath) | ConvertTo-Json -Depth 4 -Compress
""")
  cls.path=cls.root/'scripts/windows-reliability/run-token-facts.ps1';cls.path.parent.mkdir(parents=True);cls.path.write_bytes(cls.source)
  result=cls.build();cls.invocation=json.loads(result.stdout);cls.bootstrap=base64.b64decode(cls.invocation['Arguments'][4]).decode('utf-16le');cls.path.write_bytes(b'exit 97')
 @classmethod
 def tearDownClass(cls):cls.owned.cleanup()
 @classmethod
 def build(cls,mode='normal'):
  p=subprocess.run(['pwsh','-NoProfile','-File',str(cls.script),str(cls.binder),str(cls.root),mode],capture_output=True,timeout=180)
  if mode=='normal' and p.returncode:raise AssertionError((p.returncode,p.stdout.decode(),p.stderr.decode()))
  return p
 def execute(self,bootstrap=None):
  args=self.invocation['Arguments'].copy()
  if bootstrap is not None:args[4]=base64.b64encode(bootstrap.encode('utf-16le')).decode()
  return subprocess.run(['pwsh',*args],capture_output=True,timeout=60)
 def payload(self,compressed,source=None):
  script=re.sub(r"FromBase64String\('[A-Za-z0-9+/=]+'", "FromBase64String('"+base64.b64encode(compressed).decode()+"'",self.bootstrap,count=1)
  if source is not None:script=script.replace(hashlib.sha256(self.source).hexdigest(),hashlib.sha256(source).hexdigest())
  return script
 def reject(self,script):
  p=self.execute(script);self.assertEqual(p.returncode,1,p.stderr.decode(errors='replace'));self.assertEqual(p.stdout,b'');self.assertEqual(p.stderr,b'token-runner-bootstrap-failed\n')
 def test_exact_captured_runner_and_parameters_roundtrip(self):
  p=self.execute();self.assertEqual(p.returncode,0,p.stderr.decode());value=json.loads(p.stdout);self.assertEqual(value,{'directory':str(self.root),'commit':'a'*40,'output':str(self.root/'public'),'temporary':str(self.root),'runnerHash':hashlib.sha256(self.source).hexdigest()});self.assertLessEqual(len(self.invocation['Arguments'][4]),24576)
 def test_wrong_hash_rejected_before_execution(self):self.reject(self.bootstrap.replace("-cne '"+hashlib.sha256(self.source).hexdigest(),"-cne '"+'0'*64))
 def test_oversize_decompression_refused(self):
  source=self.source+b'\n#'+b'x'*262144;self.reject(self.payload(gzip.compress(source),source))
 def test_invalid_compressed_data_fixed_failure(self):self.reject(self.payload(b'PRIVATE_SENTINEL'))
 def test_changed_capture_and_relative_path_refused(self):
  for mode in ('changed','relative'):
   p=self.build(mode);self.assertNotEqual(p.returncode,0);self.assertIn(b'Token runner invocation failed',p.stderr)
 def test_command_size_refused(self):
  self.path.write_bytes(b'#'+os.urandom(100000).hex().encode());p=self.build('large');self.assertNotEqual(p.returncode,0);self.assertIn(b'Token runner invocation failed',p.stderr)

if __name__=='__main__':unittest.main(verbosity=2)
