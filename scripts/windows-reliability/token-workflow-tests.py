"""Controlled parent ordering; fake bindings/query only, no native token APIs."""
from pathlib import Path
import contextlib,hashlib,json,os,shutil,subprocess,tempfile,unittest,uuid
RUNNER=Path(__file__).with_name('run-token-facts.ps1')
@contextlib.contextmanager
def token_job():
 path=Path(tempfile.gettempdir())/('windows-reliability-token-'+uuid.uuid4().hex);path.mkdir()
 try:yield str(path)
 finally:shutil.rmtree(path)

def psq(value):return "'"+str(value).replace("'","''")+"'"
BINDER=r"""
$script:mode=MODE
$script:events=EVENTS
$script:repo=REPO
function Event($Value){[IO.File]::AppendAllText($script:events,$Value+"`n")}
function Open-CovenTokenRuntimeBinding {Event 'open';if($script:mode -eq 'open-fails'){throw 'PRIVATE_HOST_PATH'};[pscustomobject]@{Managed=[pscustomobject]@{Files=@([pscustomobject]@{Path=[Environment]::ProcessPath})}}}
function Get-CovenTokenSourceCapture {param($Directory,$Commit);Event 'capture';$files=foreach($id in @('binding','runner','query-helper','query-summary')){$path=Join-Path $Directory ('scripts/windows-reliability/'+@{'binding'='token-binding.ps1';'runner'='run-token-facts.ps1';'query-helper'='query-helper.ps1';'query-summary'='query-summary.ps1'}[$id]);$hash=(Get-FileHash $path).Hash.ToLowerInvariant();if($script:mode -eq ('wrong-'+$id)){$hash='0'*64};[pscustomobject]@{Id=$id;Path=$path;Bytes=[IO.File]::ReadAllBytes($path);Sha256=$hash}};[pscustomobject]@{Directory=$Directory;Commit=$Commit;Files=@($files)}}
function Assert-CovenTokenSourceRoot {param($Directory);if(-not [IO.Directory]::Exists($Directory)){throw 'PRIVATE_DIR'};$Directory}
function Add-CovenTokenCapturedTypes {param($Capture);Event 'compile';if($script:mode -eq 'compile-fails' -or $script:mode -in @('compile-and-close-fail','compile-close-and-outcome-fail')){throw 'PRIVATE_COMPILE'}}
function Test-CovenTokenSourceCapture {param($Capture);Event 'source-check';return $true}
function Test-CovenTokenRuntimeBinding {param($Binding);Event 'host-check';return $true}
function ConvertTo-CovenTokenSourceDto {param($Capture);Event 'source-dto';[object]::new()}
function ConvertTo-CovenTokenHostDto {param($Binding);Event 'host-dto';[object]::new()}
function New-CovenTokenProbeInvocation {param($Capture);Event 'probe-invocation';[pscustomobject]@{Arguments=@('probe')}}
function ConvertTo-CovenTokenQuerySummary {param($Query);Event 'query-summary';if($script:mode -in @('packet-fails','projection-and-cleanup-fail')){throw 'PRIVATE_PROJECTION'};[pscustomobject]@{hasCleanupFailure=($script:mode -eq 'query-cleanup-fails')}}
function New-CovenTokenPrivatePacket {param($Source,$HostProof,$Query);Event 'packet';return ,([Text.Encoding]::ASCII.GetBytes('controlled-private-packet'))}
function Close-CovenTokenRuntimeBinding {param($Binding);Event 'close';if($script:mode -in @('close-fails','compile-and-close-fail','compile-close-and-outcome-fail')){throw 'PRIVATE_RELEASE'}}
function New-CovenTokenPublisherInvocation {param($Capture,$Executable,$BundlePath,$TemporaryRoot,$OutputDirectory);Event 'publisher-invocation';[pscustomobject]@{Arguments=@('publisher',$BundlePath,$OutputDirectory)}}
"""
HELPER=r"""Add-Type -TypeDefinition @'
using System;
using System.IO;
public sealed class CovenQueryResult {
 public bool Started=true,RootExited=true,ReadersSettled=true,Reaped=true,TimedOut,OutputLimited;
 public int? ExitCode=0;public long ElapsedMs=1;
 public string Stdout="controlled-probe",Stderr="",Failure="",CleanupFailure="",ReaderFailure="";
}
public static class CovenNativeQuery {
 public static string Events=EVENTS,Mode=MODE,Repo=REPO;
 public static CovenQueryResult Run(string executable,string[] args,int budget,int cap){
  File.AppendAllText(Events,args[0]+"-query\n");var value=new CovenQueryResult();
  if(args[0]=="probe"){
   if(Mode=="component-fails")value.ExitCode=1;
   if(Mode=="projection-and-cleanup-fail")value.CleanupFailure="PRIVATE_QUERY_CLEANUP";
   return value;
  }
  if(budget!=10000||cap!=1024)throw new Exception("controlled bad budget");
  var bundle=args[1];var output=args[2];
  if(Mode!="private-retained")File.Delete(bundle);
  File.WriteAllText(Path.Combine(output,"receipt.json"),Mode=="public-changed"?"[]":"{}");
  value.Stdout="{\"sha256\":\"44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a\",\"bytes\":2}";
  if(Mode=="duplicate-publication")value.Stdout=value.Stdout.Replace("{","{\"bytes\":2,");
  if(Mode=="boolean-publication")value.Stdout=value.Stdout.Replace("\"bytes\":2","\"bytes\":true");
  if(Mode=="publication-cleanup-fails"||Mode=="child-cleanup-projection-fail"){value.Stdout="";value.Stderr="token-publication-failed cleanupFailed=true\n";value.ExitCode=1;}
  if(Mode=="publication-timeout")value.TimedOut=true;
  if(Mode=="publication-dirty")File.WriteAllText(Path.Combine(Repo,"dirty.txt"),"controlled");
  if(Mode=="component-fails")value.ExitCode=1;
  if(Mode=="exit-mismatch")value.ExitCode=2;
  if(Mode=="publisher-projection-and-cleanup-fail")value.CleanupFailure="PRIVATE_PUBLISHER_CLEANUP";
  return value;
 }
}
'@
"""
SUMMARY=r"""
function ConvertTo-CovenQueryObservation {param($Result);if($script:mode -in @('publisher-projection-and-cleanup-fail','child-cleanup-projection-fail')){throw 'PRIVATE_PUBLISHER_PROJECTION'};[pscustomobject]@{hasCleanupFailure=$false}}
"""
class ParentControls(unittest.TestCase):
 def run_case(self,mode,dirty=False):
  with tempfile.TemporaryDirectory(prefix='coven-token-parent-controlled-') as directory,token_job() as job:
   root=Path(directory);repo=root/'repo';scripts=repo/'scripts/windows-reliability';scripts.mkdir(parents=True);events=root/'events';output=Path(job)/'public';output.mkdir();github=root/'github-output'
   runner=scripts/'run-token-facts.ps1';runner.write_bytes(RUNNER.read_bytes())
   binder=BINDER.replace('MODE',psq(mode)).replace('EVENTS',psq(events)).replace('REPO',psq(repo));(scripts/'token-binding.ps1').write_text(binder)
   helper=HELPER.replace('MODE',json.dumps(mode)).replace('EVENTS',json.dumps(str(events))).replace('REPO',json.dumps(str(repo)));(scripts/'query-helper.ps1').write_text(helper);(scripts/'query-summary.ps1').write_text(SUMMARY)
   def git(*args):return subprocess.check_output(['git','-C',str(repo),'-c','core.hooksPath=/dev/null','-c','commit.gpgsign=false',*args],stderr=subprocess.PIPE,text=True).strip()
   git('init','-q');git('add','.');git('-c','user.name=Controlled Fixture','-c','user.email=fixture@example.invalid','commit','-qm','controlled runner fixture');commit=git('rev-parse','HEAD')
   if dirty:(repo/'dirty.txt').write_text('controlled')
   if mode=='compile-close-and-outcome-fail':github.mkdir()
   env=os.environ.copy();env['GITHUB_OUTPUT']=str(github)
   result=subprocess.run(['pwsh','-NoProfile','-File',str(runner),'-Directory',str(repo),'-Commit',commit,'-OutputDirectory',str(output),'-TemporaryRoot',str(Path(tempfile.gettempdir())),'-RunnerSourceHash',hashlib.sha256(runner.read_bytes()).hexdigest()],capture_output=True,env=env,timeout=120)
   self.assertNotIn(b'PRIVATE',result.stdout+result.stderr)
   text=github.read_text() if github.is_file() else '';observed=events.read_text().splitlines() if events.exists() else []
   execution=next((json.loads(line.removeprefix('execution=')) for line in text.splitlines() if line.startswith('execution=')),None)
   return result,observed,execution,text
 def failure(self,mode,stage,cleanup=False,**kwargs):
  result,events,execution,text=self.run_case(mode,**kwargs);self.assertEqual(result.returncode,1,result.stderr.decode());self.assertEqual(result.stdout,b'');self.assertEqual(result.stderr,f'token-execution-failed stage={stage} cleanupFailed={str(cleanup).lower()}\n'.encode());self.assertFalse(execution['publicationSucceeded']);self.assertEqual(execution['cleanupFailed'],cleanup);self.assertNotIn('public-receipt=',text);return events,execution
 def test_success_release_precedes_publisher_and_admitted_path(self):
  result,events,execution,text=self.run_case('normal');self.assertEqual(result.returncode,0,result.stderr.decode());self.assertTrue(execution['publicationSucceeded']);self.assertTrue(execution['referencesReleased']);self.assertEqual(execution['publication'],{'sha256':'44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a','bytes':2});self.assertIn('public-receipt=',text);self.assertLess(events.index('close'),events.index('publisher-invocation'));self.assertLess(events.index('close'),events.index('publisher-query'))
 def test_component_failure_receipt_retains_exit(self):
  result,events,execution,text=self.run_case('component-fails');self.assertEqual(result.returncode,1,result.stderr.decode());self.assertTrue(execution['publicationSucceeded']);self.assertEqual(execution['exitCode'],1);self.assertIn('public-receipt=',text)
 def test_open_failure_never_compiles(self):events,_=self.failure('open-fails','runtime-binding');self.assertNotIn('compile',events)
 def test_wrong_runner_and_binder_refuse_before_compile(self):
  for mode in ('wrong-runner','wrong-binding'):
   events,_=self.failure(mode,'source-binding');self.assertIn('close',events);self.assertNotIn('compile',events)
 def test_dirty_source_refuses_before_compile(self):events,_=self.failure('normal','source-binding',dirty=True);self.assertNotIn('compile',events)
 def test_compile_primary_survives_release_failure(self):events,_=self.failure('compile-and-close-fail','compilation',True);self.assertNotIn('publisher-query',events)
 def test_reference_release_failure_never_publishes(self):events,_=self.failure('close-fails','reference-release',True);self.assertNotIn('publisher-invocation',events)
 def test_query_cleanup_failure_never_publishes(self):events,_=self.failure('query-cleanup-fails','query-projection',True);self.assertNotIn('publisher-invocation',events)
 def test_projection_failure_never_publishes(self):events,_=self.failure('packet-fails','query-projection');self.assertNotIn('publisher-invocation',events)
 def test_publication_duplicate_and_boolean_refused(self):
  for mode in ('duplicate-publication','boolean-publication'):self.failure(mode,'publication')
 def test_publisher_cleanup_failure_retained(self):self.failure('publication-cleanup-fails','publication',True)
 def test_publisher_timeout_refused(self):self.failure('publication-timeout','publication')
 def test_private_packet_retained_withholds_artifact(self):self.failure('private-retained','private-cleanup',True)
 def test_changed_source_after_publication_refused(self):self.failure('publication-dirty','final-source')
 def test_changed_public_bytes_refused(self):self.failure('public-changed','public-receipt')
 def test_publication_exit_mismatch_refused(self):self.failure('exit-mismatch','publication')
 def test_query_cleanup_survives_projection_failure(self):self.failure('projection-and-cleanup-fail','query-projection',True)
 def test_publisher_cleanup_survives_projection_failure(self):self.failure('publisher-projection-and-cleanup-fail','publication',True)
 def test_primary_and_cleanup_survive_outcome_failure(self):
  result,events,execution,text=self.run_case('compile-close-and-outcome-fail');self.assertEqual(result.returncode,1,result.stderr.decode(errors='replace'));self.assertEqual(result.stdout,b'');self.assertEqual(result.stderr,b'token-execution-failed stage=compilation cleanupFailed=true outcomeFailed=true\n');self.assertIsNone(execution);self.assertNotIn('publisher-invocation',events)
 def test_child_cleanup_marker_survives_projection_failure(self):self.failure('child-cleanup-projection-fail','publication',True)
class TokenWorkflowContractControls(unittest.TestCase):
 def job(self,text):
  marker='\n  token-facts:\n';self.assertEqual(text.count(marker),1);job=text.split(marker,1)[1]
  self.assertEqual(job.splitlines()[0],"    if: github.event_name == 'workflow_dispatch' && inputs.stage == 'token-facts'")
  self.assertIn('options: [samples, metadata-preflight, provider-schema, token-facts]',text)
  self.assertIn('default: samples',text);self.assertNotIn('strategy:',job);self.assertNotIn('pull_request:',job)
  return job
 def check_source(self,job):
  for required in ["$env:GITHUB_WORKFLOW_SHA -ceq $head","$env:GITHUB_WORKFLOW_REF -ceq $expectedWorkflow","$ancestor -ne 0","$shallow -cne 'false'","$dirty.Count -ne 0","$binder.Sha256 -cne $binderHash","[StringComparer]::Ordinal","TOKEN_SOURCE_SHA256"]:self.assertIn(required,job)
 def check_runner(self,job):
  self.assertIn('New-CovenTokenRunnerInvocation -Capture $capture',job)
  self.assertIn('& $executable @($invocation.Arguments)',job)
  self.assertNotIn('-File scripts/windows-reliability/run-token-facts.ps1',job)
  self.assertIn("if: steps.source.outcome == 'success' && steps.controls.outcome == 'success'",job)
 def check_artifact(self,job):
  self.assertIn('New-CovenTokenArtifactInvocation -Capture $capture',job)
  self.assertIn("$result -cne 'token-artifact-admission-pass'",job)
  self.assertIn("if: always() && steps.binding.outcome == 'success' && steps.binding.outputs.receipt-admitted == 'true'",job)
  self.assertIn('path: ${{ env.TOKEN_ROOT }}/public/receipt.json',job)
  self.assertNotIn('**',job);self.assertNotIn('/private/',job);self.assertNotIn('bundle.json',job)
  self.assertIn("$env:RECEIPT_ADMITTED -cne 'true'",job)
  self.assertLess(job.index('New-CovenTokenArtifactInvocation'),job.index('"receipt-admitted='))
 def validate(self,text):
  job=self.job(text);self.check_source(job);self.check_runner(job);self.check_artifact(job);return job
 def text(self):return (Path(__file__).parents[2]/'.github/workflows/windows-reliability.yml').read_text()
 def test_manual_activation_source_executed_runner_and_artifact_contract(self):self.validate(self.text())
 def test_all_controlled_components_are_required(self):
  job=self.validate(self.text())
  for name in ['token-privileges-tests.ps1','token-reader-tests.ps1','token-native-tests.ps1','token-public-tests.ps1','token-binding-types-tests.ps1','token-binding-tests.ps1','token-runtime-tests.ps1','token_result_tests.py','token_receipt_tests.py','token_envelope_tests.py','token-workflow-tests.py']:self.assertIn("'"+name+"'",job)
 def test_wrong_activation_and_path_reread_variants_are_rejected(self):
  original=self.text();self.validate(original)
  for before,after in [("&& inputs.stage == 'token-facts'",''),('New-CovenTokenRunnerInvocation -Capture $capture','-File scripts/windows-reliability/run-token-facts.ps1'),("$env:GITHUB_WORKFLOW_SHA -ceq $head",'$true'),('New-CovenTokenArtifactInvocation -Capture $capture','unchecked-artifact'),("steps.binding.outputs.receipt-admitted == 'true'","steps.token.outputs.public-receipt != ''")]:
   self.assertIn(before,original)
   with self.assertRaises(AssertionError):self.validate(original.replace(before,after))
 def test_public_artifact_paths_are_explicit_and_closed(self):
  job=self.validate(self.text());paths=[];lines=job.splitlines()
  for index,line in enumerate(lines):
   if line.strip().startswith('path: '):paths.append(line.strip()[6:])
   if line.strip()=='path: |':
    next_index=index+1
    while next_index<len(lines) and lines[next_index].startswith('            '):paths.append(lines[next_index].strip());next_index+=1
  paths=[path for path in paths if path!='|'];self.assertEqual(set(paths),{'${{ env.TOKEN_ROOT }}/source-check.json','${{ env.TOKEN_ROOT }}/source.json','${{ env.TOKEN_ROOT }}/binding.json','${{ env.TOKEN_ROOT }}/outcome.json','${{ env.TOKEN_ROOT }}/public/receipt.json'})

class WorkflowExecutableSelectionControls(unittest.TestCase):
 def test_exact_artifact_block_selects_one_owned_executable_with_duplicate_path(self):
  text=(Path(__file__).parents[2]/'.github/workflows/windows-reliability.yml').read_text();job=text.split('\n  token-facts:\n',1)[1]
  binding=job.split('      - name: Independently recheck source before artifact admission',1)[1].split('      - name:',1)[0]
  # Run the exact executable-selection/invocation fragment, not a reimplementation.
  start=binding.index('            $python')
  end=binding.index('            $admitted=$true',start)+len('            $admitted=$true')
  fragment='\n'.join(line[12:] for line in binding[start:end].splitlines())
  with tempfile.TemporaryDirectory(prefix='coven-token-duplicate-executable-') as owned:
   root=Path(owned);first=root/'first';second=root/'second';first.mkdir();second.mkdir();events=root/'selected.json'
   windows=os.name=='nt';source=root/('fixture.cmd' if windows else 'fixture.sh');name='python.cmd' if windows else 'python'
   # Runnable owned scripts keep this fixture independent of system-binary
   # copy behavior while exercising real PATH discovery and invocation.
   source.write_bytes(b'@echo off\r\necho token-artifact-admission-pass\r\n' if windows else b'#!/bin/sh\nprintf \'%s\\n\' \'token-artifact-admission-pass\'\n')
   for directory in (first,second):
    target=directory/name;shutil.copyfile(source,target);target.chmod(0o700);self.assertFalse(target.is_symlink());self.assertEqual(target.stat().st_nlink,1);self.assertEqual(hashlib.sha256(target.read_bytes()).digest(),hashlib.sha256(source.read_bytes()).digest())
   arguments="@()"
   script=root/'selection.ps1';script.write_text(r'''param($Events)
$ErrorActionPreference='Stop'
$PSNativeCommandUseErrorActionPreference=$false
function New-CovenTokenArtifactInvocation {
 param($Capture,$Executable,$SourcePath,$SnapshotHash,$TemporaryRoot,$OutputDirectory)
 $candidates=@(Get-Command python -CommandType Application -ErrorAction Stop)
 if($candidates.Count -lt 2){throw 'Controlled duplicate executable fixture invalid'}
 if($Executable.GetType() -ne [string]){throw ('Controlled selection received array; candidates='+$candidates.Count)}
 [IO.File]::WriteAllText($Events,([ordered]@{selected=$Executable;candidates=$candidates.Count}|ConvertTo-Json -Compress))
 [pscustomobject]@{Arguments=ARGUMENTS}
}
$capture=[object]::new();$sourcePath='controlled';$admitted=$false
$env:TOKEN_SOURCE_SHA256='a'*64;$env:TOKEN_TEMP_ROOT=[IO.Path]::GetTempPath();$env:TOKEN_ROOT=$env:TOKEN_TEMP_ROOT
FRAGMENT
if(-not $admitted){throw 'Controlled artifact fragment did not admit'}
'''.replace('ARGUMENTS',arguments).replace('FRAGMENT',fragment))
   env=os.environ.copy();env['PATH']=str(first)+os.pathsep+str(second)+os.pathsep+env['PATH']
   result=subprocess.run(['pwsh','-NoProfile','-File',str(script),str(events)],capture_output=True,env=env,timeout=120)
   self.assertEqual(result.returncode,0,result.stderr.decode());selected=json.loads(events.read_bytes());self.assertEqual(Path(selected['selected']),first/name);self.assertGreaterEqual(selected['candidates'],2)

if __name__=='__main__':unittest.main(verbosity=2)
