"""Controlled strict component tests. These do not certify a native provider."""
import copy
import hashlib
import json
import unittest
import traceback
from provider_schema_receipt import validate_collector, decode_collector, decode_output, validate_export_failure, validate_exit


def digest(text):
    return hashlib.sha256(text.encode()).hexdigest()


def valid():
    return {
        'providerCode':'kernel-file','providerMatched':True,'enumerationComplete':True,
        'descriptorScope':'numeric-event-metadata','templateScope':'template-data-only-no-userdata',
        'observedEventHandles':1,'retainedEventCount':1,'privateBufferBytes':256,
        'totalProjectedFields':1,'truncated':False,
        'operation':{'errorClass':'none','nativeError':None,'failedStage':'none','failedProperty':None},
        'cleanup':{'attemptedHandles':3,'failedHandles':0,'firstNativeError':None,'attemptedBuffers':9,'failedBuffers':0,'complete':True},
        'events':[{'id':42,'version':0,'channel':0,'level':0,'opcode':0,'task':0,
                   'keywordsRawHex':'8000123456789abc','keywordsProviderHex':'0000123456789abc',
                   'template':{'status':'supported-flat','sha256':digest('controlled template'),'utf16CodeUnits':76,
                               'unsupportedCount':0,'fields':[{'ordinal':0,'knownName':'Irp','nameSha256':digest('Irp'),
                                'knownType':'win:Pointer','typeSha256':digest('win:Pointer'),
                                'literalCount':None,'literalLength':None,'unsupported':False}]}}]}


class CollectorTests(unittest.TestCase):
    def reject(self, change):
        value=valid();change(value)
        with self.assertRaises(ValueError):validate_collector(value)

    def test_valid_control(self):
        validate_collector(valid())
        self.assertEqual(decode_collector(json.dumps(valid()).encode()),valid())

    def test_unknown_keys(self):
        for path in [(),('operation',),('cleanup',),('events',0),('events',0,'template'),('events',0,'template','fields',0)]:
            def change(value):
                for part in path:value=value[part]
                value['PRIVATE_SENTINEL']='private'
            with self.subTest(path=path):self.reject(change)

    def test_numeric_types_and_ranges(self):
        for key in ['observedEventHandles','retainedEventCount','privateBufferBytes','totalProjectedFields']:
            for item in [True,-1,1.5,'1',None,1048577]:
                with self.subTest(key=key,item=item):self.reject(lambda d:d.__setitem__(key,item))
        for key,cap in [('id',65535),('version',255),('channel',255),('level',255),('opcode',255),('task',65535)]:
            for item in [True,-1,cap+1]:
                with self.subTest(key=key,item=item):self.reject(lambda d:d['events'][0].__setitem__(key,item))

    def test_boolean_types(self):
        for key in ['providerMatched','enumerationComplete','truncated']:
            self.reject(lambda d:d.__setitem__(key,1))
        self.reject(lambda d:d['cleanup'].__setitem__('complete',1))

    def test_counts_and_masks(self):
        self.reject(lambda d:d.__setitem__('retainedEventCount',0))
        self.reject(lambda d:d.__setitem__('totalProjectedFields',0))
        self.reject(lambda d:d.__setitem__('observedEventHandles',0))
        self.reject(lambda d:d['events'][0].__setitem__('keywordsProviderHex','8000123456789abc'))
        self.reject(lambda d:d['events'][0].__setitem__('keywordsProviderHex','0000000000000000'))
        self.reject(lambda d:d['events'][0].__setitem__('keywordsRawHex','0x1'))
        self.reject(lambda d:d['events'][0].__setitem__('keywordsRawHex','ABCDEF0000000000'))

    def test_template_privacy_and_consistency(self):
        field=lambda d:d['events'][0]['template']['fields'][0]
        self.reject(lambda d:field(d).__setitem__('knownName','PRIVATE_SENTINEL'))
        self.reject(lambda d:field(d).__setitem__('knownType','PRIVATE_SENTINEL'))
        self.reject(lambda d:field(d).__setitem__('nameSha256','0'*64))
        self.reject(lambda d:field(d).__setitem__('ordinal',1))
        self.reject(lambda d:field(d).__setitem__('literalCount',65536))
        self.reject(lambda d:field(d).__setitem__('knownName',None))
        self.reject(lambda d:d['events'][0]['template'].__setitem__('status','missing'))
        self.reject(lambda d:d['events'][0]['template'].__setitem__('sha256','bad'))
        self.reject(lambda d:d['events'][0]['template'].__setitem__('utf16CodeUnits',32769))
        self.reject(lambda d:d['events'][0]['template'].__setitem__('unsupportedCount',True))

    def test_primary_errors_and_cleanup_are_independent(self):
        value=valid();value['cleanup'].update(failedHandles=1,firstNativeError=5,complete=False)
        validate_collector(value)
        value['cleanup'].update(failedHandles=0,firstNativeError=None,failedBuffers=1)
        validate_collector(value)
        self.reject(lambda d:d['cleanup'].__setitem__('failedBuffers',1))
        self.reject(lambda d:d['cleanup'].__setitem__('firstNativeError',5))
        self.reject(lambda d:d['cleanup'].__setitem__('attemptedHandles',2))
        self.reject(lambda d:d['operation'].__setitem__('nativeError',5))
        self.reject(lambda d:d['operation'].__setitem__('errorClass','PRIVATE_SENTINEL'))
        self.reject(lambda d:d['operation'].__setitem__('failedStage','PRIVATE_SENTINEL'))
        self.reject(lambda d:d['operation'].__setitem__('failedProperty',7))

    def test_unavailable_is_honest_partial(self):
        value=valid();value.update(providerMatched=False,enumerationComplete=False,observedEventHandles=0,
            retainedEventCount=0,privateBufferBytes=0,totalProjectedFields=0,events=[])
        value['cleanup'].update(attemptedHandles=0,attemptedBuffers=0)
        value['operation'].update(errorClass='provider-open-failed',failedStage='provider-open',nativeError=5)
        validate_collector(value)
        value['operation']['nativeError']=True
        with self.assertRaises(ValueError):validate_collector(value)

    def test_no_events_without_provider(self):
        def unmatched(d):
            d['providerMatched']=False;d['enumerationComplete']=False
            d['operation'].update(errorClass='provider-mismatch',failedStage='provider-guid',failedProperty=0)
        self.reject(unmatched)
    def test_no_events_after_unexpected_failure(self):
        def unexpected(d):
            d['enumerationComplete']=False
            d['operation'].update(errorClass='collector-failed')
        self.reject(unexpected)

    def test_fixed_decoder_failure_has_no_underlying_diagnostic(self):
        for data in [b'\xffPRIVATE_SENTINEL',b'{']:
            try:decode_collector(data)
            except ValueError:
                rendered=traceback.format_exc()
                self.assertNotIn('UnicodeDecodeError',rendered)
                self.assertNotIn('JSONDecodeError',rendered)
                self.assertNotIn('PRIVATE_SENTINEL',rendered)
            else:self.fail('Invalid encoding accepted')

    def test_operation_stage_and_native_error_consistency(self):
        self.reject(lambda d:d['operation'].update(errorClass='provider-open-failed',failedStage='event-property'))
        self.reject(lambda d:d['operation'].update(errorClass='property-probe-failed',failedStage='event-property',failedProperty=None))
        def mismatch(d):
            d.update(providerMatched=False,enumerationComplete=False,observedEventHandles=0,retainedEventCount=0,totalProjectedFields=0,events=[])
            d['cleanup'].update(attemptedHandles=1)
            d['operation'].update(errorClass='provider-mismatch',failedStage='provider-guid',failedProperty=0,nativeError=5)
        self.reject(mismatch)

    def test_duplicate_keys_and_public_size(self):
        with self.assertRaises(ValueError):decode_collector(b'{"events":[],"events":[]}')
        with self.assertRaises(ValueError):decode_collector(b' '*524289)
        with self.assertRaises(ValueError):decode_collector(b'\xff')
        with self.assertRaises(ValueError):decode_collector(b'{"x":NaN}')
        with self.assertRaises(ValueError):decode_collector(b'['*10000)

class ExportTests(unittest.TestCase):
    def summary(self):
        value=valid()
        return {'kind':'coven.kernel-file-collector-export-failure.v1','captureAttempted':False,
                'metadataComplete':False,'collectorDataWithheld':True,'exportError':'public-size-limit',
                'collectorSummary':{k:value[k] for k in ['providerMatched','enumerationComplete','observedEventHandles','retainedEventCount','privateBufferBytes','totalProjectedFields','truncated','operation','cleanup']}}

    def test_summary_preserves_original_state(self):
        value=self.summary();validate_export_failure(value)
        self.assertEqual(decode_output(json.dumps(value).encode()),value)
        validate_exit(value,2)
        with self.assertRaises(ValueError):validate_exit(value,0)
        value['exportError']='encoding-failed';validate_exit(value,1)
        with self.assertRaises(ValueError):validate_exit(value,2)

    def test_closed_failure_summary(self):
        for change in [lambda d:d.__setitem__('events',[]),lambda d:d['collectorSummary'].__setitem__('events',[]),
                       lambda d:d.__setitem__('collectorDataWithheld',False),lambda d:d.__setitem__('captureAttempted',0),
                       lambda d:d.__setitem__('metadataComplete',True),lambda d:d.__setitem__('exportError','PRIVATE_SENTINEL'),
                       lambda d:d['collectorSummary'].__setitem__('retainedEventCount',True)]:
            value=self.summary();change(value)
            with self.assertRaises(ValueError):validate_export_failure(value)
        with self.assertRaises(ValueError):decode_output(json.dumps(self.summary()).encode()+b' '*8192)
        with self.assertRaises(ValueError):decode_output(b'{"kind":"PRIVATE_SENTINEL"}')

    def test_collector_exit_contract(self):
        value=valid();validate_exit(value,0)
        for code in [True,1,2,3]:
            with self.assertRaises(ValueError):validate_exit(value,code)
        value['cleanup'].update(failedBuffers=1,complete=False);validate_exit(value,2)
        with self.assertRaises(ValueError):validate_exit(value,0)

if __name__=='__main__':unittest.main()
