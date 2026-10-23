"""Strict read-only Kernel-File metadata decoding and bounded atomic publication.

Selected runtime/source observations are caller-trusted, not hermetic attestation.
This stage never records traces or establishes native-operation causal readiness.
"""
import hashlib
import json
import re

NAMES=frozenset('Irp IrpPtr FileObject FileKey IssuingThreadId ThreadId Status ExtraInfo InfoClass FileName FilePath CreateOptions CreateAttributes ShareAccess IoSize IoFlags ByteOffset TTID'.split())
TYPES=frozenset('win:Pointer win:UInt8 win:UInt16 win:UInt32 win:UInt64 win:Int8 win:Int16 win:Int32 win:Int64 win:UnicodeString win:AnsiString win:HexInt32 win:HexInt64 win:Binary'.split())
ERRORS=frozenset('none unsupported-platform api-unavailable host-binding-failed source-binding-failed provider-open-failed provider-guid-failed provider-mismatch enumeration-open-failed enumeration-failed event-limit property-probe-failed property-size-limit total-size-limit property-read-failed property-size-changed variant-invalid descriptor-range template-limit projection-failed public-size-limit collector-failed'.split())
STAGES=frozenset('none platform host source provider-open provider-guid enumeration-open enumeration-next event-property template public-encoding'.split())


def require(ok):
    if not ok:raise ValueError('Invalid bounded provider metadata component')


def obj(value,keys):
    require(type(value) is dict and set(value)==set(keys.split()))


def integer(value,maximum):
    require(type(value) is int and 0<=value<=maximum)


def boolean(value):
    require(type(value) is bool)


def digest(value,nullable=False):
    if nullable and value is None:return
    require(type(value) is str and re.fullmatch('[0-9a-f]{64}',value) is not None)


def enum(value,values):
    require(type(value) is str and value in values)


def template(value):
    obj(value,'status sha256 utf16CodeUnits fields unsupportedCount')
    enum(value['status'],{'missing','empty','supported-flat','unsupported','invalid','limit'})
    digest(value['sha256'],True)
    if value['utf16CodeUnits'] is not None:integer(value['utf16CodeUnits'],32768)
    integer(value['unsupportedCount'],2048)
    fields=value['fields'];require(type(fields) is list and len(fields)<=128)
    unsupported=0
    for ordinal,field in enumerate(fields):
        obj(field,'ordinal knownName nameSha256 knownType typeSha256 literalCount literalLength unsupported')
        integer(field['ordinal'],127);require(field['ordinal']==ordinal)
        boolean(field['unsupported'])
        for known,hashed,allowed in [('knownName','nameSha256',NAMES),('knownType','typeSha256',TYPES)]:
            digest(field[hashed])
            if field[known] is not None:
                enum(field[known],allowed)
                require(field[hashed]==hashlib.sha256(field[known].encode()).hexdigest())
            else:require(field['unsupported'])
        for key in ['literalCount','literalLength']:
            if field[key] is not None:integer(field[key],65535)
        unsupported+=int(field['unsupported'])
    require(value['unsupportedCount']>=unsupported)
    state=value['status']
    if state=='missing':require(value['sha256'] is None and value['utf16CodeUnits'] is None and not fields and value['unsupportedCount']==0)
    elif state=='empty':require(value['sha256']==hashlib.sha256(b'').hexdigest() and value['utf16CodeUnits']==0 and not fields and value['unsupportedCount']==0)
    elif state=='supported-flat':require(value['sha256'] is not None and value['utf16CodeUnits'] is not None and value['utf16CodeUnits']>0 and value['unsupportedCount']==0)
    elif state=='unsupported':require(value['sha256'] is not None and value['utf16CodeUnits'] is not None and value['utf16CodeUnits']>0 and value['unsupportedCount']>0)
    else:require(not fields and value['unsupportedCount']==0)
    return len(fields)


def _validate_state(value):
    for key in ['providerMatched','enumerationComplete','truncated']:boolean(value[key])
    for key,maximum in [('observedEventHandles',257),('retainedEventCount',256),('privateBufferBytes',1048576),('totalProjectedFields',2048)]:integer(value[key],maximum)
    operation=value['operation'];obj(operation,'errorClass nativeError failedStage failedProperty')
    enum(operation['errorClass'],ERRORS);enum(operation['failedStage'],STAGES)
    if operation['nativeError'] is not None:integer(operation['nativeError'],0xffffffff)
    prop=operation['failedProperty']
    if prop is not None:integer(prop,8);require(prop!=7 and operation['failedStage'] in {'provider-guid','event-property'})
    if operation['failedStage']=='provider-guid' and prop is not None:require(prop==0)
    if operation['errorClass']=='none':require(operation['nativeError'] is None and operation['failedStage']=='none' and prop is None)
    code=operation['errorClass'];stage=operation['failedStage']
    property_errors={'property-probe-failed','property-size-limit','total-size-limit','property-read-failed','property-size-changed','variant-invalid','descriptor-range'}
    fixed_stages={'none':'none','unsupported-platform':'platform','api-unavailable':'platform','host-binding-failed':'host','source-binding-failed':'source','provider-open-failed':'provider-open','provider-guid-failed':'provider-guid','provider-mismatch':'provider-guid','enumeration-open-failed':'enumeration-open','enumeration-failed':'enumeration-next','event-limit':'enumeration-next','template-limit':'template','projection-failed':'template','public-size-limit':'public-encoding'}
    if code in fixed_stages:require(stage==fixed_stages[code])
    if code in property_errors:require(stage in {'provider-guid','event-property'} and prop is not None)
    if code=='descriptor-range':require(stage=='event-property' and prop in {0,1,2,3,4,5})
    if code in {'provider-guid-failed','provider-mismatch'}:require(prop==0)
    if stage not in {'provider-guid','event-property'}:require(prop is None)
    native_failures={'provider-open-failed','provider-guid-failed','enumeration-open-failed','enumeration-failed','property-probe-failed','property-read-failed','property-size-changed'}
    if code not in native_failures:require(operation['nativeError'] is None)
    cleanup=value['cleanup'];obj(cleanup,'attemptedHandles failedHandles firstNativeError attemptedBuffers failedBuffers complete')
    for key,maximum in [('attemptedHandles',259),('failedHandles',259),('attemptedBuffers',2049),('failedBuffers',2049)]:integer(cleanup[key],maximum)
    boolean(cleanup['complete'])
    require(cleanup['failedHandles']<=cleanup['attemptedHandles'] and cleanup['failedBuffers']<=cleanup['attemptedBuffers'])
    require(cleanup['complete']==(cleanup['failedHandles']==0 and cleanup['failedBuffers']==0))
    if cleanup['firstNativeError'] is not None:
        integer(cleanup['firstNativeError'],0xffffffff);require(cleanup['failedHandles']>0 and cleanup['firstNativeError']>0)
    observed=value['observedEventHandles'];retained=value['retainedEventCount']
    require(retained<=observed and observed<=257)
    if not value['providerMatched']:require(observed==0 and retained==0 and value['totalProjectedFields']==0 and not value['enumerationComplete'])
    if operation['errorClass']=='collector-failed':require(retained==0 and value['totalProjectedFields']==0 and not value['enumerationComplete'])
    if code in {'provider-open-failed','provider-guid-failed','provider-mismatch','unsupported-platform','api-unavailable','host-binding-failed','source-binding-failed'}:require(not value['providerMatched'])
    if code in {'enumeration-open-failed','enumeration-failed','event-limit','descriptor-range','template-limit'}:require(value['providerMatched'] and not value['enumerationComplete'])
    require(observed<=cleanup['attemptedHandles']<=observed+2)
    if value['enumerationComplete']:require(value['providerMatched'] and cleanup['attemptedHandles']==observed+2)
    if operation['errorClass']=='none':require(value['providerMatched'] and value['enumerationComplete'] and not value['truncated'] and retained==observed)
    if operation['errorClass']=='event-limit':require(observed==257 and retained==256 and value['truncated'] and not value['enumerationComplete'])
    if value['truncated']:require(operation['errorClass']=='event-limit')
    return retained


def validate_collector(value):
    obj(value,'providerCode providerMatched enumerationComplete descriptorScope templateScope observedEventHandles retainedEventCount privateBufferBytes totalProjectedFields truncated operation cleanup events')
    require(value['providerCode']=='kernel-file' and value['descriptorScope']=='numeric-event-metadata' and value['templateScope']=='template-data-only-no-userdata')
    retained=_validate_state(value)
    events=value['events'];require(type(events) is list and len(events)==retained)
    fields=0
    for event in events:
        obj(event,'id version channel level opcode task keywordsRawHex keywordsProviderHex template')
        for key,maximum in [('id',65535),('version',255),('channel',255),('level',255),('opcode',255),('task',65535)]:integer(event[key],maximum)
        for key in ['keywordsRawHex','keywordsProviderHex']:require(type(event[key]) is str and re.fullmatch('[0-9a-f]{16}',event[key]) is not None)
        require(int(event['keywordsProviderHex'],16)==int(event['keywordsRawHex'],16)&0xffffffffffff)
        fields+=template(event['template'])
    require(fields==value['totalProjectedFields'] and fields<=2048)
    return value


def _pairs(pairs):
    value={}
    for key,item in pairs:
        require(key not in value);value[key]=item
    return value


def _constant(_):
    raise ValueError('Invalid bounded provider metadata component')


def decode_collector(data):
    require(type(data) is bytes and len(data)<=524288)
    try:value=json.loads(data.decode('utf-8'),object_pairs_hook=_pairs,parse_constant=_constant)
    except (UnicodeError,ValueError,RecursionError) as error:raise ValueError('Invalid bounded provider metadata component') from None
    return validate_collector(value)


def validate_export_failure(value):
    obj(value,'kind captureAttempted metadataComplete collectorDataWithheld exportError collectorSummary')
    require(value['kind']=='coven.kernel-file-collector-export-failure.v1')
    require(value['captureAttempted'] is False and value['metadataComplete'] is False and value['collectorDataWithheld'] is True)
    enum(value['exportError'],{'public-size-limit','encoding-failed'})
    summary=value['collectorSummary']
    obj(summary,'providerMatched enumerationComplete observedEventHandles retainedEventCount privateBufferBytes totalProjectedFields truncated operation cleanup')
    _validate_state(summary)
    return value


def decode_output(data):
    require(type(data) is bytes and len(data)<=524288)
    try:value=json.loads(data.decode('utf-8'),object_pairs_hook=_pairs,parse_constant=_constant)
    except (UnicodeError,ValueError,RecursionError):raise ValueError('Invalid bounded provider metadata component') from None
    if type(value) is dict and 'kind' in value:
        require(len(data)<=8192)
        return validate_export_failure(value)
    return validate_collector(value)


def validate_exit(value,code):
    integer(code,2)
    if type(value) is dict and 'kind' in value:
        validate_export_failure(value)
        require(code==(2 if value['exportError']=='public-size-limit' else 1))
    else:
        validate_collector(value)
        expected=1 if value['operation']['errorClass']=='collector-failed' else 0 if value['operation']['errorClass']=='none' and value['cleanup']['complete'] else 2
        require(code==expected)
    return value

# Fixed owning files; producer input cannot substitute a runtime-defined path list.
SOURCE_PATHS={
    'workflow':'.github/workflows/windows-reliability.yml',
    'runner':'scripts/windows-reliability/run-provider-schema.ps1',
    'probe':'scripts/windows-reliability/provider-schema-probe.ps1',
    'binding':'scripts/windows-reliability/provider-binding.ps1',
    'binding-tests':'scripts/windows-reliability/provider-binding-tests.ps1',
    'reader':'scripts/windows-reliability/provider-metadata-reader.cs',
    'template':'scripts/windows-reliability/provider-template.cs',
    'public-encoder':'scripts/windows-reliability/provider-public.cs',
    'metadata-tests':'scripts/windows-reliability/provider-metadata-tests.ps1',
    'controlled-api':'scripts/windows-reliability/provider-reader-tests.cs',
    'template-tests':'scripts/windows-reliability/provider-template-tests.ps1',
    'adapter-tests':'scripts/windows-reliability/provider-adapter-tests.ps1',
    'encoder-tests':'scripts/windows-reliability/provider-public-tests.ps1',
    'controlled-encoder':'scripts/windows-reliability/provider-public-tests.cs',
    'query-helper':'scripts/windows-reliability/query-helper.ps1',
    'query-summary':'scripts/windows-reliability/query-summary.ps1',
    'source-stability':'scripts/windows-reliability/source-stability.ps1',
    'phase-validator':'scripts/windows-reliability/public_receipt.py',
    'validator':'scripts/windows-reliability/provider_schema_receipt.py',
    'validator-tests':'scripts/windows-reliability/provider_schema_receipt_tests.py',
    'envelope-tests':'scripts/windows-reliability/provider_envelope_tests.py',
}
ASSEMBLIES=tuple(sorted(['corelib','powershell','json','xml','crypto']))
PHASE_NAMES=tuple(('startCallEnteredTicks startCallReturnedTicks stdinCloseEnteredTicks stdinCloseReturnedTicks '
    'readerSetupEnteredTicks readerSetupReturnedTicks monitorEnteredTicks timeoutObservedTicks cleanupEnteredTicks '
    'killCallEnteredTicks killCallReturnedTicks rootWaitEnteredTicks rootWaitReturnedTicks initialReaderWaitEnteredTicks '
    'initialReaderWaitReturnedTicks fallbackReaderWaitEnteredTicks fallbackReaderWaitReturnedTicks '
    'readerDisposalAttemptsEnteredTicks readerDisposalAttemptsFinishedTicks cleanupFinishedTicks').split())


def validate_source(value):
    obj(value,'commit files beforeAfterMatch')
    require(type(value['commit']) is str and re.fullmatch('[0-9a-f]{40}',value['commit']) is not None)
    require(value['beforeAfterMatch'] is True)
    files=value['files'];require(type(files) is list and len(files)==len(SOURCE_PATHS))
    ids=[]
    for item in files:
        obj(item,'id sha256');enum(item['id'],SOURCE_PATHS);digest(item['sha256']);ids.append(item['id'])
    require(ids==sorted(SOURCE_PATHS))
    return value


def validate_host(value):
    obj(value,'kind assemblyScope executableSha256 assemblies nativeLibraryKind nativeLibrarySha256 beforeAfterMatch')
    require(value['kind']=='workflow-pwsh' and value['assemblyScope']=='selected-direct-managed-assemblies')
    require(value['nativeLibraryKind']=='wevtapi-system32' and value['beforeAfterMatch'] is True)
    digest(value['executableSha256']);digest(value['nativeLibrarySha256'])
    assemblies=value['assemblies'];require(type(assemblies) is list and len(assemblies)==len(ASSEMBLIES))
    kinds=[]
    for item in assemblies:
        obj(item,'kind sha256 version');enum(item['kind'],ASSEMBLIES);digest(item['sha256'])
        require(type(item['version']) is list and len(item['version'])==4)
        for component in item['version']:integer(component,0xffffffff)
        if item['kind']=='powershell':require(item['version'][0]==7)
        kinds.append(item['kind'])
    require(kinds==list(ASSEMBLIES))
    return value


def _probe(value):
    obj(value,'schemaVersion captureAttempted metadataComplete source host collector')
    require(value['schemaVersion']=='coven.kernel-file-probe.v1' and value['captureAttempted'] is False and value['metadataComplete'] is False)
    validate_source(value['source']);validate_host(value['host'])
    collector=value['collector']
    if type(collector) is dict and 'kind' in collector:validate_export_failure(collector)
    else:validate_collector(collector)
    return value


def validate_probe(value,expected_source,expected_host):
    _probe(value);validate_source(expected_source);validate_host(expected_host)
    require(value['source']==expected_source and value['host']==expected_host)
    return value


def parse_probe(data):
    require(type(data) is bytes and len(data)<=524288)
    try:value=json.loads(data.decode('ascii'),object_pairs_hook=_pairs,parse_constant=_constant)
    except (UnicodeError,ValueError,RecursionError):raise ValueError('Invalid bounded provider metadata component') from None
    _probe(value)
    if type(value['collector']) is dict and 'kind' in value['collector']:require(len(data)<=8192)
    return value


def _phase_validator(captured,expected_hash):
    import types
    require(type(captured) is bytes and hashlib.sha256(captured).hexdigest()==expected_hash)
    module=types.ModuleType('coven_captured_phase_validator')
    module.__file__='public_receipt.py'
    # Compile the captured source, never path-import an unbound cached .pyc.
    try:
        exec(compile(captured,'public_receipt.py','exec',dont_inherit=True),module.__dict__)
        require(module.PHASE_NAMES==PHASE_NAMES and callable(module.valid_query_phases))
    except Exception:raise ValueError('Invalid bounded provider metadata component') from None
    return module.valid_query_phases


def validate_envelope(value,expected_source,expected_host,probe_data,phase_source):
    obj(value,'schemaVersion captureAttempted metadataComplete source host query collector')
    require(value['schemaVersion']=='coven.kernel-file-metadata.v1' and value['captureAttempted'] is False and value['metadataComplete'] is False)
    wire=parse_probe(probe_data);validate_probe(wire,expected_source,expected_host)
    require(value['source']==wire['source'] and value['host']==wire['host'] and value['collector']==wire['collector'])
    # Independently validate both sides; Python equality alone equates bool/int.
    validate_source(value['source']);validate_host(value['host'])
    collector=value['collector']
    if type(collector) is dict and 'kind' in collector:validate_export_failure(collector)
    else:validate_collector(collector)
    query=value['query']
    obj(query,'kind stdoutDigestScope started timedOut outputLimited rootExited readersSettled reaped exitCode elapsedMs phases hasFailure hasCleanupFailure hasReaderFailure stdoutChars stderrChars stdoutSha256 stderrSha256')
    require(query['kind']=='kernel-file-metadata' and query['stdoutDigestScope']=='decoded-text-utf8')
    for key in ['started','rootExited','readersSettled','reaped']:require(query[key] is True)
    for key in ['timedOut','outputLimited','hasFailure','hasCleanupFailure','hasReaderFailure']:require(query[key] is False)
    integer(query['elapsedMs'],0x7fffffffffffffff);integer(query['stdoutChars'],524288);integer(query['stderrChars'],524288)
    digest(query['stdoutSha256']);digest(query['stderrSha256'])
    require(query['stdoutChars']==len(probe_data) and query['stdoutSha256']==hashlib.sha256(probe_data).hexdigest())
    require(query['stderrChars']==0 and query['stderrSha256']==hashlib.sha256(b'').hexdigest())
    validate_exit(collector,query['exitCode'])
    phases=query['phases'];obj(phases,'unit frequency '+' '.join(PHASE_NAMES))
    require(phases['unit']=='stopwatch-elapsed-ticks');integer(phases['frequency'],0x7fffffffffffffff);require(phases['frequency']>0)
    for name in PHASE_NAMES:
        if phases[name] is not None:integer(phases[name],0x7fffffffffffffff)
    hashes={item['id']:item['sha256'] for item in value['source']['files']}
    require(_phase_validator(phase_source,hashes['phase-validator'])(query))
    return value


def encode_bounded_json(value,limit=524288):
    """Encode privately; never return an incomplete prefix."""
    require(type(limit) is int and 0<limit<=524288)
    result=bytearray()
    try:
        encoder=json.JSONEncoder(ensure_ascii=True,allow_nan=False,separators=(',',':'),sort_keys=True)
        for text in encoder.iterencode(value):
            part=text.encode('ascii')
            require(len(part)<=limit-len(result));result.extend(part)
        return bytes(result)
    except (ValueError,TypeError,UnicodeError,RecursionError):
        raise ValueError('Invalid bounded provider metadata component') from None


def decode_envelope(data,expected_source,expected_host,probe_data,phase_source):
    require(type(data) is bytes and len(data)<=524288)
    try:value=json.loads(data.decode('ascii'),object_pairs_hook=_pairs,parse_constant=_constant)
    except (UnicodeError,ValueError,RecursionError):raise ValueError('Invalid bounded provider metadata component') from None
    return validate_envelope(value,expected_source,expected_host,probe_data,phase_source)


def _identity(status):
    return status.st_dev,status.st_ino


def _plain_status(path,directory=False):
    import stat
    status=path.lstat()
    require(not path.is_symlink() and not path.is_junction() and not (getattr(status,'st_file_attributes',0)&0x400))
    require(stat.S_ISDIR(status.st_mode) if directory else stat.S_ISREG(status.st_mode))
    return status


def _public_directory(output_directory,temporary_root):
    from pathlib import Path
    root,output=Path(temporary_root),Path(output_directory)
    require(callable(getattr(Path,'is_junction',None)) and root.is_absolute() and output.is_absolute())
    relative=output.relative_to(root)
    require(len(relative.parts)==2 and re.fullmatch('windows-reliability-provider-[A-Za-z0-9-]+',relative.parts[0]) is not None and relative.parts[1]=='public')
    current=root.resolve(strict=True)
    chain=[(current,_identity(_plain_status(current,True)))]
    for part in relative.parts:
        current=current/part;chain.append((current,_identity(_plain_status(current,True))))
    return current,chain


def _same_directories(chain):
    for path,identity in chain:require(_identity(_plain_status(path,True))==identity)


def _remove_owned_link(path,identity,chain):
    import os
    if path is None or identity is None:return
    try:
        _same_directories(chain)
        if _identity(_plain_status(path))==identity:os.unlink(path)
    except (OSError,ValueError,RuntimeError):pass


def publish_envelope(value,expected_source,expected_host,probe_data,phase_source,output_directory,temporary_root):
    """Exclusive sibling staging, complete fsynced write, atomic no-overwrite link."""
    import os
    import uuid
    stage=target=None;owned=None;chain=[];success=False;descriptor=None
    try:
        validate_envelope(value,expected_source,expected_host,probe_data,phase_source)
        payload=encode_bounded_json(value)
        directory,chain=_public_directory(output_directory,temporary_root)
        target=directory/'receipt.json'
        # lexists also rejects a dangling foreign symlink; link itself is no-overwrite.
        require(not os.path.lexists(target))
        stage=directory/('.receipt-'+uuid.uuid4().hex+'.tmp')
        descriptor=os.open(stage,os.O_WRONLY|os.O_CREAT|os.O_EXCL|getattr(os,'O_BINARY',0)|getattr(os,'O_NOFOLLOW',0),0o600)
        opened=os.fstat(descriptor);owned=_identity(opened)
        require(opened.st_nlink==1 and _identity(_plain_status(stage))==owned)
        with os.fdopen(descriptor,'wb') as stream:
            descriptor=None
            require(stream.write(payload)==len(payload));stream.flush();os.fsync(stream.fileno())
            completed=os.fstat(stream.fileno())
            require(_identity(completed)==owned and completed.st_nlink==1 and completed.st_size==len(payload))
        _same_directories(chain)
        require(_identity(_plain_status(stage))==owned)
        os.link(stage,target,follow_symlinks=False)
        linked=_plain_status(target)
        require(_identity(linked)==owned and linked.st_nlink==2)
        _same_directories(chain)
        stage_status=_plain_status(stage)
        require(_identity(stage_status)==owned and stage_status.st_nlink==2)
        os.unlink(stage)
        _same_directories(chain)
        final=_plain_status(target)
        require(_identity(final)==owned and final.st_nlink==1 and final.st_size==len(payload))
        descriptor=os.open(target,os.O_RDONLY|getattr(os,'O_BINARY',0)|getattr(os,'O_NOFOLLOW',0))
        with os.fdopen(descriptor,'rb') as stream:
            descriptor=None;opened=os.fstat(stream.fileno())
            require(_identity(opened)==owned and opened.st_nlink==1)
            actual=stream.read(524289);after=os.fstat(stream.fileno())
        require(_identity(after)==owned and after.st_nlink==1 and after.st_size==len(payload) and after.st_mtime_ns==opened.st_mtime_ns)
        require(actual==payload)
        decode_envelope(actual,expected_source,expected_host,probe_data,phase_source)
        _same_directories(chain)
        last=_plain_status(target)
        require(_identity(last)==owned and last.st_nlink==1 and last.st_size==len(payload) and last.st_mtime_ns==after.st_mtime_ns)
        success=True
        return {'sha256':hashlib.sha256(payload).hexdigest(),'bytes':len(payload)}
    except (OSError,ValueError,RuntimeError,TypeError,RecursionError):
        raise ValueError('Provider receipt publication failed') from None
    finally:
        if descriptor is not None:os.close(descriptor)
        _remove_owned_link(stage,owned,chain)
        if not success:_remove_owned_link(target,owned,chain)


def _read_stable_regular(path,limit):
    import os
    descriptor=None
    try:
        before=_plain_status(path)
        require(before.st_nlink==1 and 0<=before.st_size<=limit)
        descriptor=os.open(path,os.O_RDONLY|getattr(os,'O_BINARY',0)|getattr(os,'O_NOFOLLOW',0))
        with os.fdopen(descriptor,'rb') as stream:
            descriptor=None;opened=os.fstat(stream.fileno())
            require(_identity(opened)==_identity(before) and opened.st_nlink==1 and opened.st_size==before.st_size)
            data=stream.read(limit+1);after=os.fstat(stream.fileno())
        require(len(data)<=limit and len(data)==after.st_size and after.st_nlink==1)
        require(_identity(after)==_identity(opened) and after.st_size==opened.st_size and after.st_mtime_ns==opened.st_mtime_ns)
        final=_plain_status(path)
        require(_identity(final)==_identity(opened) and final.st_nlink==1 and final.st_size==after.st_size and final.st_mtime_ns==after.st_mtime_ns)
        return data
    finally:
        if descriptor is not None:os.close(descriptor)


def capture_source_tree(source_root,expected_source):
    from pathlib import Path
    validate_source(expected_source)
    selected=Path(source_root);require(selected.is_absolute())
    selected_id=_identity(_plain_status(selected,True))
    root=selected.resolve(strict=True);root_id=_identity(_plain_status(root,True))
    require(root_id==selected_id)
    total=0;phase=None
    for item in expected_source['files']:
        current=root;parts=Path(SOURCE_PATHS[item['id']]).parts
        for part in parts[:-1]:current=current/part;_plain_status(current,True)
        data=_read_stable_regular(current/parts[-1],262144)
        total+=len(data);require(total<=2097152 and hashlib.sha256(data).hexdigest()==item['sha256'])
        if item['id']=='phase-validator':phase=data
    require(_identity(_plain_status(selected,True))==selected_id and
            _identity(_plain_status(root,True))==root_id and phase is not None)
    return phase


def read_provider_bundle(bundle_path,temporary_root):
    from pathlib import Path
    root,path=Path(temporary_root),Path(bundle_path)
    require(callable(getattr(Path,'is_junction',None)) and root.is_absolute() and path.is_absolute())
    relative=path.relative_to(root)
    require(len(relative.parts)==3 and re.fullmatch('coven-provider-probe-[A-Za-z0-9-]+',relative.parts[0]) is not None and relative.parts[1:]==('private','bundle.json'))
    current=root.resolve(strict=True);chain=[(current,_identity(_plain_status(current,True)))]
    for part in relative.parts[:-1]:
        current=current/part;chain.append((current,_identity(_plain_status(current,True))))
    data=_read_stable_regular(current/relative.parts[-1],1048576)
    try:value=json.loads(data.decode('ascii'),object_pairs_hook=_pairs,parse_constant=_constant)
    except (UnicodeError,ValueError,RecursionError):raise ValueError('Invalid bounded provider metadata component') from None
    obj(value,'source host query probe')
    validate_source(value['source']);validate_host(value['host'])
    require(type(value['probe']) is str and len(value['probe'])<=524288 and value['probe'].isascii())
    _same_directories(chain)
    return value


def check_source_commit(source_root,commit):
    import subprocess
    # Git and the compiler/runtime environment are caller-trusted, not hermetic.
    head=subprocess.run(['git','-C',str(source_root),'rev-parse','HEAD'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,check=True).stdout
    require(head.decode('ascii').strip()==commit)
    state=subprocess.run(['git','-C',str(source_root),'status','--porcelain=v1','--untracked-files=all'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,check=True).stdout
    require(state==b'')


def finalize_provider_bundle(bundle_path,temporary_root,output_directory,source_root):
    packet=read_provider_bundle(bundle_path,temporary_root)
    check_source_commit(source_root,packet['source']['commit'])
    phase=capture_source_tree(source_root,packet['source'])
    probe_data=packet['probe'].encode('ascii')
    wire=parse_probe(probe_data);validate_probe(wire,packet['source'],packet['host'])
    value={'schemaVersion':'coven.kernel-file-metadata.v1','captureAttempted':False,'metadataComplete':False,
        'source':packet['source'],'host':packet['host'],'query':packet['query'],'collector':wire['collector']}
    validate_envelope(value,packet['source'],packet['host'],probe_data,phase)
    # Recheck captured source before the only public publication operation.
    require(capture_source_tree(source_root,packet['source'])==phase)
    check_source_commit(source_root,packet['source']['commit'])
    result=publish_envelope(value,packet['source'],packet['host'],probe_data,phase,output_directory,temporary_root)
    return result,value['query']['exitCode']


if __name__=='__main__':
    import sys
    try:
        require(len(sys.argv)==5)
        result,code=finalize_provider_bundle(*sys.argv[1:])
    except Exception:
        print('provider-publication-failed',file=sys.stderr)
        raise SystemExit(1) from None
    print(json.dumps(result,sort_keys=True,separators=(',',':')))
    raise SystemExit(code)
