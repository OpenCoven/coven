"""Controlled strict component tests. These do not certify a native provider."""
import copy
import hashlib
import json
import unittest
import traceback
import provider_schema_receipt as receipt
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


def valid_v2():
    value=valid();value['events'][0]['template']['reasons']=[]
    return value


def unsupported_template(reasons,fields,count):
    return {'status':'unsupported','sha256':digest('controlled template'),'utf16CodeUnits':76,'unsupportedCount':count,'reasons':reasons,'fields':fields}


def field(ordinal,name,kind,unsupported):
    return {'ordinal':ordinal,'knownName':name,'nameSha256':digest(name if name is not None else 'PRIVATE_SENTINEL'),
            'knownType':kind,'typeSha256':digest(kind if kind is not None else 'PRIVATE_SENTINEL'),
            'literalCount':None,'literalLength':None,'unsupported':unsupported}


class VersionTests(unittest.TestCase):
    def test_v1_stays_strict_and_rejects_reasons(self):
        validate_collector(valid())
        with self.assertRaises(ValueError):validate_collector(valid_v2())
        with self.assertRaises(ValueError):decode_collector(json.dumps(valid_v2()).encode())
        with self.assertRaises(ValueError):decode_output(json.dumps(valid_v2()).encode())

    def test_v2_requires_reasons(self):
        validate_collector(valid_v2(),2)
        self.assertEqual(decode_collector(json.dumps(valid_v2()).encode(),2),valid_v2())
        self.assertEqual(decode_output(json.dumps(valid_v2()).encode(),2),valid_v2())
        validate_exit(valid_v2(),0,2)
        with self.assertRaises(ValueError):validate_collector(valid(),2)
        with self.assertRaises(ValueError):validate_exit(valid(),0,2)

    def test_version_is_an_explicit_closed_integer(self):
        for version in [True,0,3,'2',2.0,None]:
            with self.subTest(version=version):
                with self.assertRaises(ValueError):validate_collector(valid_v2(),version)
                with self.assertRaises(ValueError):decode_output(json.dumps(valid_v2()).encode(),version)

    def test_export_failure_is_version_independent(self):
        normal=valid()
        failure={'kind':'coven.kernel-file-collector-export-failure.v1','captureAttempted':False,'metadataComplete':False,'collectorDataWithheld':True,'exportError':'public-size-limit',
                 'collectorSummary':{k:normal[k] for k in ['providerMatched','enumerationComplete','observedEventHandles','retainedEventCount','privateBufferBytes','totalProjectedFields','truncated','operation','cleanup']}}
        for version in [1,2]:
            self.assertEqual(decode_output(json.dumps(failure).encode(),version),failure)
            validate_exit(failure,2,version)


class ReasonTests(unittest.TestCase):
    def accept(self,value):
        receipt.template(value,2)

    def reject(self,value):
        with self.assertRaises(ValueError):receipt.template(value,2)

    def test_every_status_has_its_closed_reasons(self):
        for state,codes in [('invalid',['utf16-invalid']),('invalid',['xml-invalid'])]+[('limit',[code]) for code in ['depth-limit','field-budget','field-limit','node-limit','size-limit']]:
            value={'status':state,'sha256':None,'utf16CodeUnits':10,'fields':[],'unsupportedCount':0,'reasons':codes}
            self.accept(value)
            for bad in [[],['text'],codes+codes,['xml-invalid' if state=='limit' else 'node-limit'],codes+['zzz']]:
                with self.subTest(state=state,bad=bad):self.reject({**value,'reasons':bad})
        self.reject({'status':'limit','sha256':None,'utf16CodeUnits':10,'fields':[],'unsupportedCount':0,'reasons':['depth-limit','node-limit']})
        missing={'status':'missing','sha256':None,'utf16CodeUnits':None,'fields':[],'unsupportedCount':0,'reasons':[]}
        self.accept(missing);self.reject({**missing,'reasons':['text']})
        supported=valid_v2()['events'][0]['template'];self.accept(supported)
        self.reject({**supported,'reasons':['root-attribute']})
        self.reject({k:v for k,v in supported.items() if k!='reasons'})

    def test_unsupported_reasons_are_canonical_closed_and_bounded(self):
        known=[field(0,'Irp','win:Pointer',False)]
        self.accept(unsupported_template(['root-attribute','text'],known,2))
        for bad in [[],['text','root-attribute'],['text','text'],['PRIVATE_SENTINEL'],['xml-invalid'],[1],'text',None]:
            with self.subTest(bad=bad):self.reject(unsupported_template(bad,known,2))
        everything=sorted(receipt.REASONS-{'root-name','root-namespace','root-content'})
        self.assertEqual(len(everything),12)
        fields=[field(0,None,'win:Pointer',True),field(1,'Status',None,True),field(2,'Irp','win:Pointer',True)]
        self.accept(unsupported_template(everything,fields,12))
        self.reject(unsupported_template(sorted(everything+['root-name']),fields,13))

    def test_count_is_not_reason_cardinality(self):
        fields=[field(0,None,'win:Pointer',True),field(1,None,'win:Pointer',True)]
        self.accept(unsupported_template(['field-name'],fields,2))
        self.reject(unsupported_template(['field-name'],fields,1))
        self.accept(unsupported_template(['field-attribute','field-count','field-length'],[field(0,'Irp','win:Pointer',True)],1))
        self.reject(unsupported_template(['child-element','nested-element','text'],[],2))

    def test_reasons_agree_with_exported_field_projection(self):
        known=field(0,'Irp','win:Pointer',True);supported=field(0,'Irp','win:Pointer',False)
        self.accept(unsupported_template(['field-duplicate'],[supported,{**known,'ordinal':1}],1))
        self.reject(unsupported_template(['field-name'],[known],1))
        self.reject(unsupported_template(['field-duplicate'],[field(0,None,'win:Pointer',True)],1))
        self.reject(unsupported_template(['field-type'],[known],1))
        self.reject(unsupported_template(['field-duplicate'],[field(0,'Irp',None,True)],1))
        self.accept(unsupported_template(['win-namespace'],[field(0,'Irp',None,True)],1))
        self.reject(unsupported_template(['field-duplicate'],[supported],1))
        self.reject(unsupported_template(['root-attribute'],[known],1))
        self.accept(unsupported_template(['nested-element'],[known],1))
        self.accept(unsupported_template(['text'],[known],1))

    def test_root_codes_exclude_fields_and_each_other(self):
        self.accept(unsupported_template(['root-content','root-name'],[],2))
        self.accept(unsupported_template(['root-attribute','root-content','root-namespace','text'],[],4))
        self.reject(unsupported_template(['root-content'],[],1))
        self.reject(unsupported_template(['root-name','root-namespace'],[],2))
        self.reject(unsupported_template(['root-name'],[field(0,'Irp','win:Pointer',False)],1))
        self.reject(unsupported_template(['child-element','root-name'],[],2))


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
