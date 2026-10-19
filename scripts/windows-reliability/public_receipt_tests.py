import copy
import json
import os
from pathlib import Path
import tempfile
import subprocess
import unittest
from unittest.mock import patch
from public_receipt import validate_public_receipt, parse_public_receipt, read_public_receipt, publish_public_receipt


def fixture():
    tools = ['xperf','tracelog','tracerpt','WPAExporter','wevtutil','whoami']
    queries = ['xperf-start-help','xperf-providers-help','xperf-loggers','xperf-providers','xperf-profint','xperf-processing-help','xperf-dumper-help','xperf-tracestats-help','xperf-cswitch-help','kernel-file-schema','kernel-file-schema-no-messages','token-groups','token-privileges']
    files = ['run-preflight.ps1','query-helper.ps1','query-plan.ps1','query-summary.ps1','query-executor.ps1','tool-identity.ps1','tool-discovery.ps1','provider-schema.ps1','preflight-core.ps1','receipt-writer.ps1','source-stability.ps1']
    return dict(formatVersion=1,captureAttempted=False,metadataComplete=False,
        pending=['provider-schema-decoding','token-summary','controller-capability-decoding','outer-workflow-guard','native-validation'],
        tools=[dict(tool=t,locationKind='path',present=False,identityFailed=False,sha256=None,fileVersion=None,signatureStatus='unavailable',microsoftSigner=False,originalFilenameMatched=False,admitted=False) for t in tools],
        queries=[dict(id=q,available=False,executed=False,skippedAfterCleanupFailure=False) for q in queries],
        providerSchemas=[dict(id=q,parsed=False) for q in queries[9:11]],aborted=False,
        clock=dict(isHighResolution=True,frequency=100,before=1,utc='2026-10-18T00:00:00.0000000Z',after=2),
        sourceFiles=[dict(file=f,sha256='0'*64) for f in files],sourceStable=True)


class PublicReceiptTests(unittest.TestCase):
    def test_accepts_partial_failed_attempt_receipts(self):
        value=fixture();validate_public_receipt(value)
        value['sourceStable']=False;value['aborted']=True
        validate_public_receipt(value)

    def test_rejects_nested_private_data_and_wrong_scalar_types(self):
        original=fixture()
        mutations=[lambda v:v.update(rawResults={'secret':'private'}),
            lambda v:v['tools'][0].update(path='/private/SECRET'),
            lambda v:v['queries'][0].update(stdout='SECRET'),
            lambda v:v['clock'].update(utc='/private/SECRET'),
            lambda v:v.update(formatVersion=True),
            lambda v:v['clock'].update(frequency=False),
            lambda v:v['sourceFiles'][0].update(sha256='SECRET'),
            lambda v:v['queries'].__setitem__(1,copy.deepcopy(v['queries'][0]))]
        for mutation in mutations:
            value=copy.deepcopy(original);mutation(value)
            with self.subTest(value=value),self.assertRaises(ValueError):validate_public_receipt(value)

    def test_decoder_rejects_hidden_duplicate_keys_and_non_json_numbers(self):
        raw=json.dumps(fixture())
        self.assertEqual(parse_public_receipt(raw.encode()),fixture())
        bad=[raw.replace('"sourceStable": true','"sourceStable": "SECRET private path", "sourceStable": true'),
             raw.replace('"frequency": 100','"frequency": NaN'),
             raw.replace('"frequency": 100','"frequency": Infinity')]
        for text in bad:
            with self.assertRaises(ValueError):parse_public_receipt(text.encode())
        for data in [b'\xff',b' '*(8*1024*1024+1),b'['*2000+b']'*2000]:
            with self.assertRaises(ValueError):parse_public_receipt(data)


    def test_only_reads_exact_owned_public_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);run=root/'coven-native-preflight-test'
            public=run/'public';private=run/'private';public.mkdir(parents=True);private.mkdir()
            receipt=public/'receipt.json';receipt.write_text(json.dumps(fixture()),encoding='utf-8')
            self.assertEqual(read_public_receipt(receipt,root),fixture())
            hidden=private/'receipt.json';hidden.write_text(json.dumps(fixture()),encoding='utf-8')
            for path in [hidden,public/'other.json',public/'missing.json']:
                with self.assertRaises(ValueError):read_public_receipt(path,root)
            receipt.unlink();receipt.symlink_to(hidden)
            with self.assertRaises(ValueError):read_public_receipt(receipt,root)
            receipt.unlink();os.link(hidden,receipt)
            with self.assertRaises(ValueError):read_public_receipt(receipt,root)
            receipt.unlink();receipt.write_bytes(b' '*(8*1024*1024+1))
            with self.assertRaises(ValueError):read_public_receipt(receipt,root)


    def test_publication_uses_validated_values_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);run=root/'coven-native-preflight-test'
            public=run/'public';public.mkdir(parents=True);(run/'private').mkdir()
            source=public/'receipt.json';source.write_text(json.dumps(fixture()),encoding='utf-8')
            output=root/'windows-reliability-metadata-test'/'public';output.mkdir(parents=True)
            result=publish_public_receipt(source,root,output)
            target=output/'receipt.json';before=target.read_bytes()
            self.assertEqual(parse_public_receipt(before),fixture())
            self.assertRegex(result['sha256'],r'^[0-9a-f]{64}$')
            with self.assertRaises(ValueError):publish_public_receipt(source,root,output)
            self.assertEqual(target.read_bytes(),before)
            other=root/'windows-reliability-metadata-other'/'public';other.mkdir(parents=True)
            source.write_text(json.dumps(fixture()).replace('"sourceStable": true','"sourceStable": "SECRET", "sourceStable": true'),encoding='utf-8')
            with self.assertRaises(ValueError):publish_public_receipt(source,root,other)
            self.assertFalse((other/'receipt.json').exists())


    def test_missing_junction_inspection_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);run=root/'coven-native-preflight-test'
            public=run/'public';public.mkdir(parents=True);(run/'private').mkdir()
            source=public/'receipt.json';source.write_text(json.dumps(fixture()),encoding='utf-8')
            output=root/'windows-reliability-metadata-test'/'public';output.mkdir(parents=True)
            # Deletion exposes PathBase's callable fallback in Python 3.13.
            # Mask it explicitly so this host truly lacks the capability.
            with patch.object(Path,'is_junction',None):
                self.assertFalse(callable(getattr(Path,'is_junction',None)))
                with self.assertRaises(ValueError):read_public_receipt(source,root)
                with self.assertRaises(ValueError):publish_public_receipt(source,root,output)
                self.assertFalse((output/'receipt.json').exists())


    def test_workflow_publication_uses_the_query_temporary_root(self):
        repo=Path(__file__).resolve().parents[2]
        lines=(repo/'.github/workflows/windows-reliability.yml').read_text(encoding='utf-8').splitlines()
        def workflow_step(name):
            start=lines.index('      - name: '+name)
            end=next((i for i in range(start+1,len(lines)) if lines[i].startswith('      - ')),len(lines))
            begin=lines.index('        run: |',start,end)+1
            return '\n'.join(line[10:] for line in lines[begin:end])
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);native_temp=root/'native-temp';runner_temp=root/'runner-temp'
            native_temp.mkdir();runner_temp.mkdir()
            env=dict(os.environ,TMPDIR=str(native_temp),TMP=str(native_temp),TEMP=str(native_temp),
                     RUNNER_TEMP=str(runner_temp),GITHUB_RUN_ID='publication-test',GITHUB_RUN_ATTEMPT='1',
                     GITHUB_ENV=str(root/'github-env'))
            def run_script(name,script,*arguments):
                path=root/(name+'.ps1')
                if not script.startswith('param('):script="$ErrorActionPreference='Stop'\n"+script
                path.write_text(script,encoding='utf-8')
                result=subprocess.run(['pwsh','-NoProfile','-File',str(path),*map(str,arguments)],
                                      cwd=repo,env=env,capture_output=True,text=True)
                self.assertEqual(result.returncode,0,result.stdout+result.stderr)
                return result.stdout
            run_script('prepare',workflow_step('Prepare isolated public evidence directory'))
            for line in (root/'github-env').read_text(encoding='utf-8-sig').splitlines():
                key,separator,value=line.partition('=');self.assertEqual(separator,'=');env[key]=value
            fixture_path=root/'fixture.json';fixture_path.write_text(json.dumps(fixture()),encoding='utf-8')
            paths=json.loads(run_script('writer',"""
param([string]$Writer,[string]$Fixture)
$ErrorActionPreference='Stop'
. $Writer
$result=[pscustomobject]@{public=(Get-Content -LiteralPath $Fixture -Raw | ConvertFrom-Json);rawResults=@{}}
Write-CovenPreflightReceipt -Result $result | ConvertTo-Json -Compress
""".lstrip(),repo/'scripts/windows-reliability/receipt-writer.ps1',fixture_path))
            source=Path(paths['publicDirectory'])/'receipt.json'
            self.assertEqual(Path(paths['root']).parent.resolve(),native_temp.resolve())
            with self.assertRaises(ValueError):read_public_receipt(source,runner_temp)
            expected=parse_public_receipt(source.read_bytes())
            env['PUBLIC_SOURCE_RECEIPT']=str(source)
            run_script('publish',workflow_step('Validate and publish only the public receipt'))
            output=Path(env['EXPERIMENT_ROOT']);published=(output/'receipt.json').read_bytes()
            self.assertEqual(parse_public_receipt(published),expected)
            publication=json.loads((output/'publication.json').read_text(encoding='utf-8-sig'))
            import hashlib
            self.assertEqual(publication['sha256'],hashlib.sha256(published).hexdigest())


    @unittest.skipUnless(os.name=='nt','native Windows junction execution required')
    def test_native_windows_intermediate_junctions_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);run=root/'coven-native-preflight-test'
            public=run/'public';public.mkdir(parents=True);(run/'private').mkdir()
            source=public/'receipt.json';source.write_text(json.dumps(fixture()),encoding='utf-8')
            output=root/'windows-reliability-metadata-test'/'public';output.mkdir(parents=True)
            cmd=str(Path(os.environ['SystemRoot'])/'System32'/'cmd.exe')
            source_alias=root/'coven-native-preflight-link'
            output_alias=root/'windows-reliability-metadata-link'
            subprocess.run([cmd,'/d','/c','mklink','/J',str(source_alias),str(run)],check=True,capture_output=True)
            subprocess.run([cmd,'/d','/c','mklink','/J',str(output_alias),str(output.parent)],check=True,capture_output=True)
            self.assertTrue(source_alias.is_junction());self.assertTrue(output_alias.is_junction())
            with self.assertRaises(ValueError):read_public_receipt(source_alias/'public'/'receipt.json',root)
            with self.assertRaises(ValueError):publish_public_receipt(source,root,output_alias/'public')
            self.assertFalse((output/'receipt.json').exists())


    def test_rejects_unreviewed_schema_strings(self):
        value=fixture();value['providerSchemas'][0]=dict(id='kernel-file-schema',parsed=True,schema=dict(providerMatched=True,parseFailed=False,unsupportedEvents=0,schemaComplete=False,events=[dict(id=21,version=1,templateFieldsFound=True,fields=[dict(name='Irp',nameSha256='0'*64,inType='win:Pointer',typeSha256='0'*64)])]))
        validate_public_receipt(value)
        value['providerSchemas'][0]['schema']['events'][0]['fields'][0]['name']='SECRET private-path'
        with self.assertRaises(ValueError):validate_public_receipt(value)


if __name__=='__main__':unittest.main()
