"""Portable token-result contract controls; no native token API is invoked."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest
import subprocess
import tempfile

PATH=Path(__file__).with_name('token_result.py')
spec=importlib.util.spec_from_file_location('controlled_token_result',PATH)
module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def result():
    return {'failure':0,'nativeError':0,'cleanupFailed':False,'clearAttempts':6,'clearFailures':0,'freeAttempts':6,'freeFailures':0,'closeAttempts':2,'closeFailures':0,'facts':{'context':1,'tokenType':1,'impersonationLevel':None,'elevationType':3,'elevated':False,'endpointsMatch':True,'privilege':{'present':False,'enabled':False,'enabledByDefault':False,'removed':False,'usedForAccess':False}}}


def wire():
    return {'kind':'coven.effective-token-result.v1','captureAttempted':False,'privilegeName':'SeSystemProfilePrivilege','result':result()}


class TokenResultTests(unittest.TestCase):
    def reject(self,value):
        with self.assertRaisesRegex(ValueError,'^Invalid bounded token result$'):
            module.validate_component(value)

    def test_process_and_thread_success(self):
        value=wire();self.assertEqual(module.load_component(json.dumps(value).encode()),value)
        for level in range(4):
            value=wire();value['result']['facts'].update(context=0,tokenType=2,impersonationLevel=level)
            module.validate_component(value)

    def test_disabled_enabled_default_and_absence(self):
        for enabled,by_default in [(False,False),(True,False),(False,True),(True,True)]:
            value=wire();value['result']['facts']['privilege'].update(present=True,enabled=enabled,enabledByDefault=by_default)
            module.validate_component(value)
        for name in ['enabled','enabledByDefault','removed','usedForAccess']:
            value=wire();value['result']['facts']['privilege'][name]=True;self.reject(value)

    def test_every_primary_failure_withholds_facts(self):
        for failure in range(1,11):
            value=wire();value['result'].update(failure=failure,facts=None)
            module.validate_component(value)
            value['result']['facts']=result()['facts'];self.reject(value)

    def test_cleanup_withholds_without_overwriting_primary(self):
        for failure in [0,1,4,7,10]:
            for count in ['clearFailures','freeFailures','closeFailures']:
                value=wire();value['result'].update(failure=failure,cleanupFailed=True,facts=None);value['result'][count]=1
                module.validate_component(value)
                value['result']['cleanupFailed']=False;self.reject(value)
        value=wire();value['result']['facts']=None;self.reject(value)

    def test_failure_and_integer_type_bounds(self):
        for key,maximum in [('failure',10),('nativeError',0xffffffff),('clearAttempts',6),('freeAttempts',6),('closeAttempts',2),('clearFailures',6),('freeFailures',6),('closeFailures',2)]:
            for invalid in [-1,maximum+1,True,False,1.0,'1',None]:
                value=wire();value['result'][key]=invalid;self.reject(value)

    def test_exact_attempts_and_cleanup_relationships(self):
        for key in ['clearAttempts','freeAttempts','closeAttempts']:
            value=wire();value['result'][key]-=1;self.reject(value)
        for failed,attempted in [('clearFailures','clearAttempts'),('freeFailures','freeAttempts'),('closeFailures','closeAttempts')]:
            value=wire();value['result'].update(failure=7,facts=None,cleanupFailed=True);value['result'][failed]=1;value['result'][attempted]=0;self.reject(value)
        value=wire();value['result'].update(failure=7,facts=None,clearAttempts=5);self.reject(value)

    def test_cleanup_error_does_not_short_circuit_attempts(self):
        for key in ['clearAttempts','freeAttempts','closeAttempts']:
            value=wire();value['result'].update(cleanupFailed=True,clearFailures=1,facts=None)
            if key in ['clearAttempts','freeAttempts']:value['result'].update(clearAttempts=5,freeAttempts=5)
            else:value['result'][key]=1
            self.reject(value)
        for key,maximum in [('clearAttempts',6),('freeAttempts',6),('closeAttempts',2)]:
            value=wire();value['result'].update(failure=7,facts=None)
            value['result'][key]=maximum+1
            if key in ['clearAttempts','freeAttempts']:value['result'].update(clearAttempts=7,freeAttempts=7)
            self.reject(value)

    def test_native_error_failure_classes(self):
        for failure in range(11):
            value=wire();value['result'].update(failure=failure,nativeError=5,facts=None)
            if failure in [1,2,4,5,7]:module.validate_component(value)
            else:self.reject(value)

    def test_context_type_levels(self):
        for change in [{'context':2},{'context':False},{'tokenType':2},{'impersonationLevel':0},{'context':0,'tokenType':1},{'context':0,'tokenType':2,'impersonationLevel':4},{'context':0,'tokenType':2,'impersonationLevel':True}]:
            value=wire();value['result']['facts'].update(change);self.reject(value)
        for invalid in [0,4,True,'3']:
            value=wire();value['result']['facts']['elevationType']=invalid;self.reject(value)

    def test_exact_booleans_and_endpoint_match(self):
        for key in ['elevated','endpointsMatch']:
            for invalid in [0,1,'true',None]:
                value=wire();value['result']['facts'][key]=invalid;self.reject(value)
        value=wire();value['result']['facts']['endpointsMatch']=False;self.reject(value)
        for key in result()['facts']['privilege']:
            value=wire();value['result']['facts']['privilege'][key]=1;self.reject(value)

    def test_exact_keys_and_closed_strings(self):
        for path in [[],['result'],['result','facts'],['result','facts','privilege']]:
            value=wire();item=value
            for key in path:item=item[key]
            item['privateIdentifier']='PRIVATE_TOKEN_SENTINEL';self.reject(value)
        for key,bad in [('kind','coven.kernel-file-probe.v1'),('privilegeName','SeDebugPrivilege'),('captureAttempted',True),('captureAttempted',0)]:
            value=wire();value[key]=bad;self.reject(value)

    def test_strict_whole_wire(self):
        valid=json.dumps(wire()).encode()
        for invalid in [valid+b' '*max(0,4097-len(valid)),valid.replace(b'"kind": ',b'"kind": "coven.effective-token-result.v1", "kind": ',1),valid.replace(b'"nativeError": 0',b'"nativeError": 0, "nativeError": 0'),valid+b'{}',valid[:-1],b'\xff',b'\xef\xbb\xbf'+valid,b' '*4097,b'{"kind":"x","kind":"y"}',valid.replace(b'"failure": 0',b'"failure": NaN')]:
            with self.assertRaisesRegex(ValueError,'^Invalid bounded token result$'):module.load_component(invalid)

    def test_actual_typed_encoder_crosses_decoder(self):
        with tempfile.TemporaryDirectory(prefix='coven-token-wire-controlled-') as temporary:
            directory=Path(temporary)/'fixtures'
            run=subprocess.run(['pwsh','-NoProfile','-File',str(Path(__file__).with_name('token-public-tests.ps1')),'-FixtureDirectory',str(directory)],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,timeout=30)
            self.assertEqual(run.returncode,0,run.stdout.decode('utf-8',errors='replace'))
            for name,exit_code in [('success',0),('primary-cleanup',1),('output-limit',2),('serialization-failed',2)]:
                with self.subTest(name=name):
                    value=module.load_component((directory/(name+'.json')).read_bytes())
                    self.assertEqual(module.expected_exit(value),exit_code)
                    if name!='success':self.assertEqual((value.get('result') or value['resultSummary'])['failure'],7)

    def test_export_failure_preserves_summary(self):
        summary=result();summary.pop('facts')
        value={'kind':'coven.effective-token-export-failure.v1','captureAttempted':False,'factsWithheld':True,'exportError':'output-limit','resultSummary':summary}
        module.validate_component(value)
        for failure in [1,7,10]:
            value=copy.deepcopy(value);value['resultSummary'].update(failure=failure,cleanupFailed=True,clearFailures=1);module.validate_component(value)
        for key,bad in [('factsWithheld',False),('exportError','PRIVATE_EXCEPTION_SENTINEL')]:
            changed=copy.deepcopy(value);changed[key]=bad;self.reject(changed)
        changed=copy.deepcopy(value);changed['resultSummary']['facts']=result()['facts'];self.reject(changed)


if __name__=='__main__':unittest.main()
