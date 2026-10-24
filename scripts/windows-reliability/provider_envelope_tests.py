"""Controlled integration-schema tests; no native calls or publication."""
import copy
import subprocess
import sys
import shutil
import tempfile
import os
from unittest.mock import patch
import provider_schema_receipt as receipt
import hashlib
import json
from pathlib import Path
import unittest
from provider_schema_receipt import validate_probe, validate_envelope, parse_probe
from provider_schema_receipt_tests import valid, valid_v2

PHASE_SOURCE=Path(__file__).with_name('public_receipt.py').read_bytes()
IDS=sorted('workflow runner probe binding binding-tests reader template public-encoder metadata-tests controlled-api template-tests adapter-tests encoder-tests controlled-encoder query-helper query-summary source-stability phase-validator validator validator-tests envelope-tests'.split())


def source():
    return {'commit':'1'*40,'beforeAfterMatch':True,'files':[{'id':name,'sha256':hashlib.sha256(PHASE_SOURCE).hexdigest() if name=='phase-validator' else '0'*64} for name in IDS]}


def host():
    return {'kind':'workflow-pwsh','assemblyScope':'selected-direct-managed-assemblies','executableSha256':'2'*64,
            'assemblies':[{'kind':name,'sha256':'3'*64,'version':[7 if name=='powershell' else 8,0,0,0]} for name in sorted(['corelib','powershell','json','xml','crypto'])],
            'nativeLibraryKind':'wevtapi-system32','nativeLibrarySha256':'4'*64,'beforeAfterMatch':True}


def probe(version=1):
    return {'schemaVersion':f'coven.kernel-file-probe.v{version}','captureAttempted':False,'metadataComplete':False,
            'source':source(),'host':host(),'collector':valid() if version==1 else valid_v2()}


def query_for(encoded,exit_code=0):
    names=('startCallEnteredTicks startCallReturnedTicks stdinCloseEnteredTicks stdinCloseReturnedTicks readerSetupEnteredTicks readerSetupReturnedTicks monitorEnteredTicks timeoutObservedTicks cleanupEnteredTicks killCallEnteredTicks killCallReturnedTicks rootWaitEnteredTicks rootWaitReturnedTicks initialReaderWaitEnteredTicks initialReaderWaitReturnedTicks fallbackReaderWaitEnteredTicks fallbackReaderWaitReturnedTicks readerDisposalAttemptsEnteredTicks readerDisposalAttemptsFinishedTicks cleanupFinishedTicks').split()
    phases={'unit':'stopwatch-elapsed-ticks','frequency':1000,**dict.fromkeys(names)}
    present=[n for n in names if n not in {'timeoutObservedTicks','killCallEnteredTicks','killCallReturnedTicks','fallbackReaderWaitEnteredTicks','fallbackReaderWaitReturnedTicks'}]
    phases.update({name:i for i,name in enumerate(present)})
    return {'kind':'kernel-file-metadata','stdoutDigestScope':'decoded-text-utf8','started':True,'timedOut':False,'outputLimited':False,
            'rootExited':True,'readersSettled':True,'reaped':True,'exitCode':exit_code,'elapsedMs':20,'phases':phases,
            'hasFailure':False,'hasCleanupFailure':False,'hasReaderFailure':False,'stdoutChars':len(encoded),'stderrChars':0,
            'stdoutSha256':hashlib.sha256(encoded).hexdigest(),'stderrSha256':hashlib.sha256(b'').hexdigest()}


def envelope(version=1):
    wire=probe(version);encoded=json.dumps(wire,separators=(',',':')).encode()
    names=('startCallEnteredTicks startCallReturnedTicks stdinCloseEnteredTicks stdinCloseReturnedTicks readerSetupEnteredTicks readerSetupReturnedTicks monitorEnteredTicks timeoutObservedTicks cleanupEnteredTicks killCallEnteredTicks killCallReturnedTicks rootWaitEnteredTicks rootWaitReturnedTicks initialReaderWaitEnteredTicks initialReaderWaitReturnedTicks fallbackReaderWaitEnteredTicks fallbackReaderWaitReturnedTicks readerDisposalAttemptsEnteredTicks readerDisposalAttemptsFinishedTicks cleanupFinishedTicks').split()
    phases={'unit':'stopwatch-elapsed-ticks','frequency':1000,**dict.fromkeys(names)}
    present=[n for n in names if n not in {'timeoutObservedTicks','killCallEnteredTicks','killCallReturnedTicks','fallbackReaderWaitEnteredTicks','fallbackReaderWaitReturnedTicks'}]
    phases.update({name:i for i,name in enumerate(present)})
    query={'kind':'kernel-file-metadata','stdoutDigestScope':'decoded-text-utf8','started':True,'timedOut':False,'outputLimited':False,
           'rootExited':True,'readersSettled':True,'reaped':True,'exitCode':0,'elapsedMs':20,'phases':phases,
           'hasFailure':False,'hasCleanupFailure':False,'hasReaderFailure':False,'stdoutChars':len(encoded),'stderrChars':0,
           'stdoutSha256':hashlib.sha256(encoded).hexdigest(),'stderrSha256':hashlib.sha256(b'').hexdigest()}
    return {'schemaVersion':f'coven.kernel-file-metadata.v{version}','captureAttempted':False,'metadataComplete':False,
            'source':source(),'host':host(),'query':query,'collector':wire['collector']},encoded


class SourceRootTests(unittest.TestCase):
    def fixture(self,tree):
        tree.mkdir(parents=True)
        expected=source()
        for item in expected['files']:
            path=tree/receipt.SOURCE_PATHS[item['id']];path.parent.mkdir(parents=True,exist_ok=True)
            data=PHASE_SOURCE if item['id']=='phase-validator' else b'controlled source'
            path.write_bytes(data);item['sha256']=hashlib.sha256(data).hexdigest()
        self.assertEqual(receipt.capture_source_tree(tree,expected),PHASE_SOURCE)
        return expected

    def alias(self,path,target):
        if os.name=='nt':
            # A junction needs no developer-mode symlink privilege. Paths are
            # environment data, never interpolated into PowerShell source.
            import base64
            script="$ErrorActionPreference='Stop';New-Item -ItemType Junction -Path $env:COVEN_TEST_ALIAS -Target $env:COVEN_TEST_TARGET | Out-Null"
            result=subprocess.run(['pwsh','-NoProfile','-EncodedCommand',base64.b64encode(script.encode('utf-16le')).decode('ascii')],
                env={**os.environ,'COVEN_TEST_ALIAS':str(path),'COVEN_TEST_TARGET':str(target)},capture_output=True,timeout=30)
            self.assertEqual(result.returncode,0,'Owned junction fixture setup failed')
            self.assertTrue(path.is_junction(),'Owned junction fixture was not a junction')
        else:
            path.symlink_to(target,target_is_directory=True)
            self.assertTrue(path.is_symlink())

    def remove_alias(self,path):
        if os.name=='nt':path.rmdir()
        else:path.unlink()

    def test_selected_root_alias_is_rejected_with_complete_identical_source(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder).resolve();tree=root/'checkout';expected=self.fixture(tree)
            alias=root/'selected';self.alias(alias,tree)
            try:
                with patch.object(receipt,'_read_stable_regular',wraps=receipt._read_stable_regular) as read:
                    with self.assertRaises(ValueError):receipt.capture_source_tree(alias,expected)
                    read.assert_not_called()
            finally:self.remove_alias(alias)
            self.assertEqual(receipt.capture_source_tree(tree,expected),PHASE_SOURCE)

    def test_selected_plain_root_identity_is_rechecked_after_ancestor_retarget(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder).resolve();tree=root/'first'/'checkout';expected=self.fixture(tree)
            second=root/'second';shutil.copytree(tree.parent,second)
            self.assertEqual(receipt.capture_source_tree(second/'checkout',expected),PHASE_SOURCE)
            alias=root/'parent';self.alias(alias,tree.parent);selected=alias/'checkout'
            self.assertEqual(receipt.capture_source_tree(selected,expected),PHASE_SOURCE)
            original=receipt._read_stable_regular;count=0
            def retarget_after_last_read(path,limit):
                nonlocal count
                data=original(path,limit);count+=1
                if count==len(expected['files']):
                    self.remove_alias(alias);self.alias(alias,second)
                return data
            try:
                with patch.object(receipt,'_read_stable_regular',side_effect=retarget_after_last_read):
                    with self.assertRaises(ValueError):receipt.capture_source_tree(selected,expected)
                self.assertEqual(count,len(expected['files']))
            finally:self.remove_alias(alias)
            self.assertEqual(receipt.capture_source_tree(tree,expected),PHASE_SOURCE)


class EnvelopeTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which('pwsh'), 'PowerShell parent packet boundary required')
    def test_actual_parent_typed_identity_packet_matches_strict_decoder(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder).resolve()
            private=root/'coven-provider-probe-control'/'private';private.mkdir(parents=True)
            bundle=private/'bundle.json'
            script=Path(__file__).with_name('provider-binding-tests.ps1')
            result=subprocess.run(['pwsh','-NoProfile','-File',str(script),'-BundleOutputFile',str(bundle)],capture_output=True,timeout=60)
            self.assertEqual(result.returncode,0,result.stderr.decode())
            packet=json.loads(bundle.read_bytes())
            self.assertEqual(set(packet['source']), {'commit','files','beforeAfterMatch'})
            self.assertEqual(set(packet['host']),set(host()))
            self.assertEqual(receipt.read_provider_bundle(bundle,root),packet)
            self.assertEqual(sorted(item['id'] for item in packet['source']['files']),IDS)
            self.assertEqual(packet['probe'],'{}')

    def test_valid_controlled_bindings_and_query(self):
        validate_probe(probe(),source(),host())
        validate_probe(parse_probe(json.dumps(probe()).encode()),source(),host())
        value,wire=envelope();validate_envelope(value,source(),host(),wire,PHASE_SOURCE)

    def test_source_inventory_and_expected_identity(self):
        for change in [lambda d:d['source']['files'].pop(),lambda d:d['source']['files'].append(d['source']['files'][0]),
                       lambda d:d['source']['files'][0].__setitem__('id','PRIVATE_SENTINEL'),lambda d:d['source'].__setitem__('commit','2'*40),
                       lambda d:d['source'].__setitem__('beforeAfterMatch',False),lambda d:d['source']['files'][0].__setitem__('sha256','5'*64)]:
            value=probe();change(value)
            with self.assertRaises(ValueError):validate_probe(value,source(),host())

    def test_selected_host_identity(self):
        for change in [lambda d:d['host']['assemblies'].pop(),lambda d:d['host']['assemblies'][0].__setitem__('kind','eventlog'),
                       lambda d:d['host'].__setitem__('assemblyScope','all-runtime'),lambda d:d['host'].__setitem__('beforeAfterMatch',False),
                       lambda d:d['host'].__setitem__('executableSha256','5'*64),lambda d:d['host'].__setitem__('nativeLibrarySha256','5'*64),
                       lambda d:d['host']['assemblies'][0]['version'].__setitem__(0,True)]:
            value=probe();change(value)
            with self.assertRaises(ValueError):validate_probe(value,source(),host())

    def test_query_cleanup_exit_digest_and_phase_binding(self):
        for change in [lambda d:d['query'].__setitem__('kind','xperf-start-help'),lambda d:d['query'].__setitem__('stdoutDigestScope','raw-wire'),
                       lambda d:d['query'].__setitem__('rootExited',False),lambda d:d['query'].__setitem__('readersSettled',False),
                       lambda d:d['query'].__setitem__('reaped',False),lambda d:d['query'].__setitem__('hasCleanupFailure',True),
                       lambda d:d['query'].__setitem__('exitCode',2),lambda d:d['query'].__setitem__('stdoutSha256','0'*64),
                       lambda d:d['query'].__setitem__('stdoutChars',0),lambda d:d['query']['phases'].__setitem__('cleanupFinishedTicks',-1),
                       lambda d:d['query']['phases'].__setitem__('startCallReturnedTicks',None)]:
            value,wire=envelope();change(value)
            with self.assertRaises(ValueError):validate_envelope(value,source(),host(),wire,PHASE_SOURCE)
        value,wire=envelope()
        with self.assertRaises(ValueError):validate_envelope(value,source(),host(),wire,PHASE_SOURCE+b'\n')
        with self.assertRaises(ValueError):validate_envelope(value,source(),host(),wire+b' ',PHASE_SOURCE)

    def test_probe_fallback_keeps_separate_small_cap(self):
        value=probe();normal=value['collector']
        value['collector']={'kind':'coven.kernel-file-collector-export-failure.v1','captureAttempted':False,'metadataComplete':False,
            'collectorDataWithheld':True,'exportError':'public-size-limit','collectorSummary':{k:normal[k] for k in ['providerMatched','enumerationComplete','observedEventHandles','retainedEventCount','privateBufferBytes','totalProjectedFields','truncated','operation','cleanup']}}
        parse_probe(json.dumps(value).encode())
        with self.assertRaises(ValueError):parse_probe(json.dumps(value).encode()+b' '*8192)

    def test_private_bundle_and_source_tree_binding(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder).resolve();tree=root/'checkout';tree.mkdir()
            expected=source()
            for item in expected['files']:
                path=tree/receipt.SOURCE_PATHS[item['id']];path.parent.mkdir(parents=True,exist_ok=True)
                data=PHASE_SOURCE if item['id']=='phase-validator' else b'controlled source'
                path.write_bytes(data);item['sha256']=hashlib.sha256(data).hexdigest()
            self.assertEqual(receipt.capture_source_tree(tree,expected),PHASE_SOURCE)
            wire=probe();wire['source']=expected;raw=json.dumps(wire,separators=(',',':')).encode()
            value,unused=envelope();value['source']=expected
            value['query']['stdoutChars']=len(raw);value['query']['stdoutSha256']=hashlib.sha256(raw).hexdigest()
            private=root/'coven-provider-probe-control'/'private';private.mkdir(parents=True)
            bundle=private/'bundle.json'
            packet={'source':expected,'host':host(),'query':value['query'],'probe':raw.decode('ascii')}
            bundle.write_bytes(json.dumps(packet).encode())
            self.assertEqual(receipt.read_provider_bundle(bundle,root),packet)
            out=root/'windows-reliability-provider-control'/'public';out.mkdir(parents=True)
            with patch.object(receipt,'check_source_commit'):
                result,code=receipt.finalize_provider_bundle(bundle,root,out,tree)
            self.assertEqual(code,0);self.assertEqual(result['bytes'],(out/'receipt.json').stat().st_size)
            alias=root/'bundle-alias';os.link(bundle,alias)
            with self.assertRaises(ValueError):receipt.read_provider_bundle(bundle,root)
            alias.unlink()
            changed=tree/receipt.SOURCE_PATHS['reader'];changed.write_bytes(b'changed')
            with self.assertRaises(ValueError):receipt.capture_source_tree(tree,expected)
            changed.write_bytes(b'controlled source');os.link(changed,root/'source-alias')
            with self.assertRaises(ValueError):receipt.capture_source_tree(tree,expected)

    def test_atomic_publication_and_no_overwrite(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder).resolve();out=root/'windows-reliability-provider-control'/'public';out.mkdir(parents=True)
            value,wire=envelope()
            result=receipt.publish_envelope(value,source(),host(),wire,PHASE_SOURCE,out,root)
            data=(out/'receipt.json').read_bytes()
            self.assertEqual(result,{'sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)})
            self.assertEqual((out/'receipt.json').stat().st_nlink,1)
            self.assertEqual(list(out.iterdir()),[out/'receipt.json'])
            receipt.decode_envelope(data,source(),host(),wire,PHASE_SOURCE)
            with self.assertRaisesRegex(ValueError,'Provider receipt publication failed'):
                receipt.publish_envelope(value,source(),host(),wire,PHASE_SOURCE,out,root)
            self.assertEqual((out/'receipt.json').read_bytes(),data)

    def test_atomic_failure_never_deletes_foreign_replacement(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder).resolve();out=root/'windows-reliability-provider-control'/'public';out.mkdir(parents=True)
            value,wire=envelope()
            with patch('os.link',side_effect=OSError('PRIVATE_SENTINEL')):
                with self.assertRaises(ValueError):receipt.publish_envelope(value,source(),host(),wire,PHASE_SOURCE,out,root)
            self.assertEqual(list(out.iterdir()),[])
            original=os.unlink
            def replace_after_stage(path,*args,**kwargs):
                if Path(path).name.startswith('.receipt-'):
                    original(out/'receipt.json');(out/'receipt.json').write_bytes(b'FOREIGN')
                return original(path,*args,**kwargs)
            with patch('os.unlink',side_effect=replace_after_stage):
                with self.assertRaises(ValueError):receipt.publish_envelope(value,source(),host(),wire,PHASE_SOURCE,out,root)
            self.assertEqual((out/'receipt.json').read_bytes(),b'FOREIGN')

    def test_publication_preserves_foreign_stage_replacement(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder).resolve();out=root/'windows-reliability-provider-control'/'public';out.mkdir(parents=True)
            value,wire=envelope();original=receipt._plain_status;stages=[]
            def replace_after_target_check(path,directory=False):
                status=original(path,directory)
                if Path(path).name=='receipt.json' and status.st_nlink==2 and not stages:
                    stage=list(out.glob('.receipt-*.tmp'))[0]
                    stage.unlink();stage.write_bytes(b'FOREIGN_STAGE');stages.append(stage)
                return status
            with patch.object(receipt,'_plain_status',side_effect=replace_after_target_check):
                with self.assertRaises(ValueError):receipt.publish_envelope(value,source(),host(),wire,PHASE_SOURCE,out,root)
            self.assertEqual(stages[0].read_bytes(),b'FOREIGN_STAGE')
            self.assertFalse((out/'receipt.json').exists())

    def test_publication_containment_symlinks_hardlinks_and_caps(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder).resolve();out=root/'windows-reliability-provider-control'/'public';out.mkdir(parents=True)
            value,wire=envelope()
            foreign=root/'foreign';foreign.mkdir()
            out.rmdir();out.symlink_to(foreign,target_is_directory=True)
            with self.assertRaises(ValueError):receipt.publish_envelope(value,source(),host(),wire,PHASE_SOURCE,out,root)
            self.assertEqual(list(foreign.iterdir()),[])
            out.unlink();out.mkdir()
            original=os.link
            alias=root/'alias'
            def extra_link(src,dst,*args,**kwargs):
                original(src,dst,*args,**kwargs);original(src,alias)
            with patch('os.link',side_effect=extra_link):
                with self.assertRaises(ValueError):receipt.publish_envelope(value,source(),host(),wire,PHASE_SOURCE,out,root)
            self.assertTrue(alias.exists())
            self.assertEqual(len(receipt.encode_bounded_json('x'*524286)),524288)
            with self.assertRaises(ValueError):receipt.encode_bounded_json('x'*524287)
            with self.assertRaises(ValueError):receipt.decode_envelope(b' '*524289,source(),host(),wire,PHASE_SOURCE)

    @unittest.skipIf(sys.platform=='win32' or shutil.which('pwsh') is None,'portable unsupported-host control')
    def test_probe_unsupported_host_emits_only_closed_failure(self):
        script=Path(__file__).with_name('provider-schema-probe.ps1')
        result=subprocess.run(['pwsh','-NoProfile','-File',str(script),'-Directory',str(script.parent.parent.parent),'-Commit','1'*40],capture_output=True,timeout=15)
        self.assertEqual(result.returncode,2)
        self.assertEqual(result.stdout,b'')
        self.assertEqual(result.stderr,b'provider-binding-failed\n')

    @unittest.skipUnless(shutil.which('pwsh'), 'PowerShell error-tail control required')
    def test_parent_error_tail_keeps_primary_stage_and_withholds_output(self):
        # Execute the real catch/finally/output tail with controlled failures; no DLL,
        # child process, private collector wire or publisher is invoked.
        runner=Path(__file__).with_name('run-provider-schema.ps1').read_text()
        marker="} catch {\n    $failed=$true;$published=$false"
        self.assertEqual(runner.count(marker),1)
        tail=runner[runner.index(marker):]
        for primary,cleanup,expected in [(True,False,'public-validation'),(True,True,'public-validation'),(False,True,'reference-release')]:
            with self.subTest(primary=primary,cleanup=cleanup), tempfile.TemporaryDirectory() as folder:
                root=Path(folder).resolve();output=root/'step-output'
                setup=("$ErrorActionPreference='Stop';$binding=[pscustomobject]@{};$query=$null;$summary=$null;"
                       "$sourceBound=$false;$hostBound=$false;$published=$true;$failed=$false;"
                       "$cleanupFailed=$false;$stage='public-validation';$failureStage='none';$code=0;\n")
                release="throw 'PRIVATE_CLEANUP_SENTINEL'" if cleanup else ''
                body="throw 'PRIVATE_PRIMARY_SENTINEL'" if primary else ''
                script=root/'controlled-tail.ps1'
                script.write_text(setup+'function Close-CovenProviderRuntimeBinding { param($Binding) '+release+' }\ntry { '+body+'\n'+tail)
                result=subprocess.run(['pwsh','-NoProfile','-File',str(script)],capture_output=True,timeout=15,env={**os.environ,'GITHUB_OUTPUT':str(output)})
                self.assertEqual(result.returncode,1)
                text=output.read_text();self.assertNotIn('public-receipt=',text)
                execution=json.loads(text.removeprefix('execution='))
                self.assertEqual(execution['failureStage'],expected)
                self.assertEqual(execution['cleanupFailed'],cleanup)
                self.assertFalse(execution['publicationSucceeded'])
                self.assertEqual(execution['exitCode'],1)
                self.assertEqual(result.stdout,b'')
                expected_error=b'provider-execution-cleanup-failed\n' if cleanup else b'provider-execution-failed\n'
                self.assertEqual(result.stderr,expected_error)
                self.assertNotIn('PRIVATE_',text)

    @unittest.skipIf(sys.platform=='win32' or shutil.which('pwsh') is None,'portable unsupported-host control')
    def test_runner_unsupported_host_has_no_public_artifact(self):
        script=Path(__file__).with_name('run-provider-schema.ps1')
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder).resolve();out=root/'windows-reliability-provider-control'/'public';out.mkdir(parents=True)
            environment=os.environ.copy();environment['GITHUB_OUTPUT']=str(root/'outputs')
            result=subprocess.run(['pwsh','-NoProfile','-File',str(script),'-Directory',str(script.parent.parent.parent),'-Commit','1'*40,'-OutputDirectory',str(out),'-TemporaryRoot',str(root)],capture_output=True,timeout=15,env=environment)
            output=(root/'outputs').read_text()
            self.assertTrue(output.startswith('execution='));self.assertNotIn('public-receipt=',output)
            execution=json.loads(output.removeprefix('execution='))
            self.assertEqual(execution,{'kind':'coven.kernel-file-execution.v1','captureAttempted':False,'metadataComplete':False,'sourceBound':False,'hostBound':False,'cleanupFailed':False,'failureStage':'runtime-binding','query':None,'publicationSucceeded':False,'exitCode':1})
            self.assertNotIn(str(root),output)
            self.assertEqual(result.returncode,1)
            self.assertEqual(result.stdout,b'')
            self.assertEqual(result.stderr,b'provider-execution-failed\n')
            self.assertEqual(list(out.iterdir()),[])

    def test_probe_is_distinct_bounded_and_strict(self):
        value=probe();value['query']={}
        with self.assertRaises(ValueError):parse_probe(json.dumps(value).encode())
        value=probe();value['schemaVersion']='coven.kernel-file-metadata.v1'
        with self.assertRaises(ValueError):parse_probe(json.dumps(value).encode())
        with self.assertRaises(ValueError):parse_probe(b' '*524289)
        with self.assertRaises(ValueError):parse_probe(b'{"schemaVersion":1,"schemaVersion":2}')
        with self.assertRaises(ValueError):parse_probe(b'\xef\xbb\xbf{}')

class VersionedEnvelopeTests(unittest.TestCase):
    def test_v2_probe_and_envelope_validate(self):
        validate_probe(probe(2),source(),host())
        validate_probe(parse_probe(json.dumps(probe(2)).encode()),source(),host())
        value,wire=envelope(2);validate_envelope(value,source(),host(),wire,PHASE_SOURCE)

    def test_versions_are_selected_only_by_exact_schema(self):
        mixed=probe(1);mixed['collector']=valid_v2()
        bare=probe(2);bare['collector']=valid()
        for value in [mixed,bare]:
            with self.assertRaises(ValueError):parse_probe(json.dumps(value).encode())
        for schema in ['coven.kernel-file-probe.v3','coven.kernel-file-probe.V2','coven.kernel-file-metadata.v2',['coven.kernel-file-probe.v2'],None,2]:
            value=probe(2);value['schemaVersion']=schema
            with self.subTest(schema=schema):
                with self.assertRaises(ValueError):parse_probe(json.dumps(value).encode())
        for metadata,wire_version in [(1,2),(2,1)]:
            value,unused=envelope(metadata);raw=json.dumps(probe(wire_version),separators=(',',':')).encode()
            value['collector']=probe(wire_version)['collector'];value['query']=query_for(raw)
            with self.subTest(metadata=metadata,wire=wire_version):
                with self.assertRaises(ValueError):validate_envelope(value,source(),host(),raw,PHASE_SOURCE)
        value,wire=envelope(2);value['schemaVersion']=['coven.kernel-file-metadata.v2']
        with self.assertRaises(ValueError):validate_envelope(value,source(),host(),wire,PHASE_SOURCE)

    def test_finalizer_keeps_the_decoded_probe_version(self):
        for version in [1,2]:
            with self.subTest(version=version), tempfile.TemporaryDirectory() as folder:
                root=Path(folder).resolve();tree=root/'checkout';tree.mkdir()
                expected=source()
                for item in expected['files']:
                    path=tree/receipt.SOURCE_PATHS[item['id']];path.parent.mkdir(parents=True,exist_ok=True)
                    data=PHASE_SOURCE if item['id']=='phase-validator' else b'controlled source'
                    path.write_bytes(data);item['sha256']=hashlib.sha256(data).hexdigest()
                wire=probe(version);wire['source']=expected;raw=json.dumps(wire,separators=(',',':')).encode()
                private=root/'coven-provider-probe-control'/'private';private.mkdir(parents=True)
                bundle=private/'bundle.json'
                bundle.write_bytes(json.dumps({'source':expected,'host':host(),'query':query_for(raw),'probe':raw.decode('ascii')}).encode())
                out=root/'windows-reliability-provider-control'/'public';out.mkdir(parents=True)
                with patch.object(receipt,'check_source_commit'):
                    result,code=receipt.finalize_provider_bundle(bundle,root,out,tree)
                published=json.loads((out/'receipt.json').read_bytes())
                self.assertEqual(code,0);self.assertEqual(result['bytes'],(out/'receipt.json').stat().st_size)
                self.assertEqual(published['schemaVersion'],f'coven.kernel-file-metadata.v{version}')
                self.assertEqual('reasons' in published['collector']['events'][0]['template'],version==2)


if __name__=='__main__':unittest.main()
