"""Strict owned-child token decoding and bounded receipt publication; no native operation.

Source/host identities are selected endpoint observations, not hermetic
attestation, recording permission, or proof of another process's token.
"""
import hashlib
import json
import re
import types

SOURCE_PATHS={'binding': 'scripts/windows-reliability/token-binding.ps1',
 'binding-tests': 'scripts/windows-reliability/token-binding-tests.ps1',
 'binding-types': 'scripts/windows-reliability/token-binding-types.cs',
 'binding-types-tests': 'scripts/windows-reliability/token-binding-types-tests.ps1',
 'envelope-tests': 'scripts/windows-reliability/token_envelope_tests.py',
 'native-adapter': 'scripts/windows-reliability/token-native.cs',
 'native-controls': 'scripts/windows-reliability/token-native-controls.cs',
 'native-tests': 'scripts/windows-reliability/token-native-tests.ps1',
 'phase-validator': 'scripts/windows-reliability/public_receipt.py',
 'privilege-controls': 'scripts/windows-reliability/token-privileges-controls.cs',
 'privilege-projector': 'scripts/windows-reliability/token-privileges.cs',
 'privilege-tests': 'scripts/windows-reliability/token-privileges-tests.ps1',
 'probe': 'scripts/windows-reliability/token-facts-probe.ps1',
 'public-controls': 'scripts/windows-reliability/token-public-controls.cs',
 'public-encoder': 'scripts/windows-reliability/token-public.cs',
 'public-tests': 'scripts/windows-reliability/token-public-tests.ps1',
 'query-helper': 'scripts/windows-reliability/query-helper.ps1',
 'query-summary': 'scripts/windows-reliability/query-summary.ps1',
 'reader': 'scripts/windows-reliability/token-reader.cs',
 'reader-controls': 'scripts/windows-reliability/token-reader-controls.cs',
 'reader-tests': 'scripts/windows-reliability/token-reader-tests.ps1',
 'result-tests': 'scripts/windows-reliability/token_result_tests.py',
 'result-validator': 'scripts/windows-reliability/token_result.py',
 'runner': 'scripts/windows-reliability/run-token-facts.ps1',
 'runtime-tests': 'scripts/windows-reliability/token-runtime-tests.ps1',
 'source-stability': 'scripts/windows-reliability/source-stability.ps1',
 'validator': 'scripts/windows-reliability/token_receipt.py',
 'validator-tests': 'scripts/windows-reliability/token_receipt_tests.py',
 'workflow': '.github/workflows/windows-reliability.yml',
 'workflow-controls': 'scripts/windows-reliability/token-workflow-tests.py'}
ASSEMBLIES=('corelib','crypto','json','powershell')
LIBRARIES=('advapi32-system32','kernel32-system32')


def require(ok):
    if not ok:raise ValueError('Invalid bounded token receipt')


def obj(value,keys):
    require(type(value) is dict and set(value)==set(keys.split()))


def integer(value,maximum):
    require(type(value) is int and 0<=value<=maximum)


def digest(value):
    require(type(value) is str and re.fullmatch('[0-9a-f]{64}',value) is not None)


def _identity_cap(value):
    require(len(json.dumps(value,ensure_ascii=True,separators=(',',':')).encode('ascii'))<=8192)


def validate_source(value):
    obj(value,'commit files beforeAfterMatch')
    require(type(value['commit']) is str and re.fullmatch('[0-9a-f]{40}',value['commit']) is not None)
    require(value['beforeAfterMatch'] is True)
    files=value['files'];require(type(files) is list and len(files)==len(SOURCE_PATHS))
    ids=[]
    for item in files:
        obj(item,'id sha256');require(type(item['id']) is str and item['id'] in SOURCE_PATHS);digest(item['sha256']);ids.append(item['id'])
    require(ids==sorted(SOURCE_PATHS));_identity_cap(value)
    return value


def validate_host(value):
    obj(value,'kind assemblyScope executableSha256 assemblies nativeLibraries beforeAfterMatch')
    require(value['kind']=='workflow-pwsh' and value['assemblyScope']=='selected-direct-managed-assemblies' and value['beforeAfterMatch'] is True)
    digest(value['executableSha256'])
    assemblies=value['assemblies'];require(type(assemblies) is list and len(assemblies)==len(ASSEMBLIES))
    kinds=[]
    for item in assemblies:
        obj(item,'kind sha256 version');require(type(item['kind']) is str and item['kind'] in ASSEMBLIES);digest(item['sha256'])
        version=item['version'];require(type(version) is list and len(version)==4)
        for part in version:integer(part,0xffffffff)
        if item['kind']=='powershell':require(version[0]==7)
        kinds.append(item['kind'])
    require(kinds==list(ASSEMBLIES))
    libraries=value['nativeLibraries'];require(type(libraries) is list and len(libraries)==len(LIBRARIES));kinds=[]
    for item in libraries:
        obj(item,'kind sha256');require(type(item['kind']) is str and item['kind'] in LIBRARIES);digest(item['sha256']);kinds.append(item['kind'])
    require(kinds==list(LIBRARIES));_identity_cap(value)
    return value


def _unique(pairs):
    value={}
    for key,item in pairs:
        require(key not in value);value[key]=item
    return value


def _constant(_):
    require(False)


def _decode(data,limit):
    require(type(data) is bytes and 0<len(data)<=limit)
    return json.loads(data.decode('utf-8',errors='strict'),object_pairs_hook=_unique,parse_constant=_constant)


def _component_decoder(captured,expected_hash):
    require(type(captured) is bytes and 0<len(captured)<=262144 and hashlib.sha256(captured).hexdigest()==expected_hash)
    module=types.ModuleType('coven_captured_token_result')
    module.__file__='token_result.py'
    # Execute the exact captured source, never an ambient cached sibling .pyc.
    exec(compile(captured,'token_result.py','exec',dont_inherit=True),module.__dict__)
    require(callable(module.load_component) and callable(module.expected_exit))
    return module


def decode_probe(data,expected_source,expected_host,component_source):
    try:
        value=_decode(data,32768)
        obj(value,'kind captureAttempted source host component')
        require(value['kind']=='coven.effective-token-probe.v1' and value['captureAttempted'] is False)
        validate_source(value['source']);validate_host(value['host'])
        # Validate each side independently: Python equality alone equates bool/int.
        validate_source(expected_source);validate_host(expected_host)
        require(value['source']==expected_source and value['host']==expected_host)
        text=value['component'];require(type(text) is str)
        raw=text.encode('utf-8',errors='strict');require(0<len(raw)<=4096)
        hashes={item['id']:item['sha256'] for item in value['source']['files']}
        decoder=_component_decoder(component_source,hashes['result-validator'])
        component=decoder.load_component(raw)
        code=decoder.expected_exit(component);integer(code,2)
        return value,component,code
    except Exception:
        raise ValueError('Invalid bounded token receipt') from None

PHASE_NAMES=tuple(('startCallEnteredTicks startCallReturnedTicks stdinCloseEnteredTicks stdinCloseReturnedTicks '
    'readerSetupEnteredTicks readerSetupReturnedTicks monitorEnteredTicks timeoutObservedTicks cleanupEnteredTicks '
    'killCallEnteredTicks killCallReturnedTicks rootWaitEnteredTicks rootWaitReturnedTicks initialReaderWaitEnteredTicks '
    'initialReaderWaitReturnedTicks fallbackReaderWaitEnteredTicks fallbackReaderWaitReturnedTicks '
    'readerDisposalAttemptsEnteredTicks readerDisposalAttemptsFinishedTicks cleanupFinishedTicks').split())


def _phase_validator(captured,expected_hash):
    require(type(captured) is bytes and 0<len(captured)<=262144 and hashlib.sha256(captured).hexdigest()==expected_hash)
    module=types.ModuleType('coven_captured_token_phases');module.__file__='public_receipt.py'
    exec(compile(captured,'public_receipt.py','exec',dont_inherit=True),module.__dict__)
    require(module.PHASE_NAMES==PHASE_NAMES and callable(module.valid_query_phases))
    return module.valid_query_phases


def encode_bounded_json(value,limit=65536):
    """Encode privately; an over-cap or failed encoding returns no prefix."""
    try:
        require(type(limit) is int and 0<limit<=65536)
        result=bytearray()
        encoder=json.JSONEncoder(ensure_ascii=True,allow_nan=False,separators=(',',':'),sort_keys=True)
        for text in encoder.iterencode(value):
            part=text.encode('ascii');require(len(part)<=limit-len(result));result.extend(part)
        return bytes(result)
    except Exception:
        raise ValueError('Invalid bounded token receipt') from None


def validate_envelope(value,expected_source,expected_host,probe_data,component_source,phase_source):
    try:
        obj(value,'kind captureAttempted metadataComplete observationScope source host query component')
        require(value['kind']=='coven.effective-token-facts.v1' and value['captureAttempted'] is False and value['metadataComplete'] is False)
        require(value['observationScope']=='owned-child-effective-token')
        wire,component,code=decode_probe(probe_data,expected_source,expected_host,component_source)
        validate_source(value['source']);validate_host(value['host'])
        require(value['source']==wire['source'] and value['host']==wire['host'])
        hashes={item['id']:item['sha256'] for item in value['source']['files']}
        # Validate the exported object independently before equality: bool/int
        # equality is insufficient for admitting a second serialization boundary.
        decoder=_component_decoder(component_source,hashes['result-validator'])
        public_component=decoder.load_component(encode_bounded_json(value['component'],4096))
        require(public_component==component)
        query=value['query']
        obj(query,'kind stdoutDigestScope started timedOut outputLimited rootExited readersSettled reaped exitCode elapsedMs phases hasFailure hasCleanupFailure hasReaderFailure stdoutChars stderrChars stdoutSha256 stderrSha256')
        require(query['kind']=='effective-token-facts' and query['stdoutDigestScope']=='decoded-text-utf8')
        for key in ['started','rootExited','readersSettled','reaped']:require(query[key] is True)
        for key in ['timedOut','outputLimited','hasFailure','hasCleanupFailure','hasReaderFailure']:require(query[key] is False)
        integer(query['exitCode'],2);require(query['exitCode']==code)
        integer(query['elapsedMs'],0x7fffffffffffffff);integer(query['stdoutChars'],32768);integer(query['stderrChars'],32768)
        digest(query['stdoutSha256']);digest(query['stderrSha256'])
        # The fixed C# encoder emits ASCII JSON; then .NET UTF-16 character
        # accounting equals this byte count. This is decoded-text digest scope.
        require(probe_data.isascii())
        require(query['stdoutChars']==len(probe_data) and query['stdoutSha256']==hashlib.sha256(probe_data).hexdigest())
        require(query['stderrChars']==0 and query['stderrSha256']==hashlib.sha256(b'').hexdigest())
        phases=query['phases'];obj(phases,'unit frequency '+' '.join(PHASE_NAMES))
        require(phases['unit']=='stopwatch-elapsed-ticks');integer(phases['frequency'],0x7fffffffffffffff);require(phases['frequency']>0)
        for name in PHASE_NAMES:
            if phases[name] is not None:integer(phases[name],0x7fffffffffffffff)
        require(_phase_validator(phase_source,hashes['phase-validator'])(query))
        # Bound the whole complete public object even for direct callers.
        encode_bounded_json(value)
        return value
    except Exception:
        raise ValueError('Invalid bounded token receipt') from None


def decode_envelope(data,expected_source,expected_host,probe_data,component_source,phase_source):
    try:
        value=_decode(data,65536)
        return validate_envelope(value,expected_source,expected_host,probe_data,component_source,phase_source)
    except Exception:
        raise ValueError('Invalid bounded token receipt') from None


def decode_packet(data,expected_source,expected_host,component_source,phase_source):
    """Admit the parent transport privately; return only a complete public value."""
    try:
        packet=_decode(data,131072);obj(packet,'source host query probe')
        validate_source(packet['source']);validate_host(packet['host'])
        validate_source(expected_source);validate_host(expected_host)
        require(packet['source']==expected_source and packet['host']==expected_host)
        text=packet['probe'];require(type(text) is str and 0<len(text)<=32768 and text.isascii())
        raw=text.encode('ascii')
        wire,component,code=decode_probe(raw,expected_source,expected_host,component_source)
        value={'kind':'coven.effective-token-facts.v1','captureAttempted':False,'metadataComplete':False,
            'observationScope':'owned-child-effective-token','source':wire['source'],'host':wire['host'],
            'query':packet['query'],'component':component}
        validate_envelope(value,expected_source,expected_host,raw,component_source,phase_source)
        # The final transport is independently decoded too; no packet/probe text
        # or half-encoded public object escapes this admission function.
        admitted=decode_envelope(encode_bounded_json(value),expected_source,expected_host,raw,component_source,phase_source)
        return admitted,code
    except Exception:
        raise ValueError('Invalid bounded token receipt') from None

class TokenPublicationFailure(ValueError):
    def __init__(self,cleanup_failed=False):
        super().__init__('Token receipt publication failed')
        self.cleanup_failed=bool(cleanup_failed)


def _identity(status):
    return status.st_dev,status.st_ino


def _plain_status(path,directory=False):
    import stat
    from pathlib import Path
    require(callable(getattr(Path,'is_junction',None)))
    status=path.lstat()
    require(not path.is_symlink() and not path.is_junction() and not (getattr(status,'st_file_attributes',0)&0x400))
    require(stat.S_ISDIR(status.st_mode) if directory else stat.S_ISREG(status.st_mode))
    return status


def _same_directories(chain):
    for path,identity in chain:require(_identity(_plain_status(path,True))==identity)


def _read_stable_regular(path,limit):
    import os
    descriptor=None
    try:
        before=_plain_status(path);require(before.st_nlink==1 and 0<=before.st_size<=limit)
        descriptor=os.open(path,os.O_RDONLY|getattr(os,'O_BINARY',0)|getattr(os,'O_NOFOLLOW',0))
        with os.fdopen(descriptor,'rb') as stream:
            descriptor=None;opened=os.fstat(stream.fileno())
            require(_identity(opened)==_identity(before) and opened.st_nlink==1 and opened.st_size==before.st_size and opened.st_mtime_ns==before.st_mtime_ns)
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
    root=Path(source_root);require(root.is_absolute())
    # Check the selected root itself before any resolution can hide an alias.
    # Ordinary platform aliases in its ancestors are not rejected.
    chain={root:_identity(_plain_status(root,True))};total=0;captured={}
    for item in expected_source['files']:
        current=root;parts=Path(SOURCE_PATHS[item['id']]).parts
        for part in parts[:-1]:
            current=current/part;identity=_identity(_plain_status(current,True))
            if current in chain:require(chain[current]==identity)
            else:chain[current]=identity
        data=_read_stable_regular(current/parts[-1],262144)
        total+=len(data);require(total<=2097152 and hashlib.sha256(data).hexdigest()==item['sha256'])
        if item['id'] in {'result-validator','phase-validator'}:captured[item['id']]=data
    _same_directories(chain.items());require(set(captured)=={'result-validator','phase-validator'})
    return captured


def check_source_commit(source_root,commit):
    import subprocess
    # Git and the caller environment are trusted; this is not hermetic execution.
    head=subprocess.run(['git','-C',str(source_root),'rev-parse','HEAD'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,check=True).stdout
    require(head.decode('ascii').strip()==commit)
    state=subprocess.run(['git','-C',str(source_root),'status','--porcelain=v1','--untracked-files=all'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,check=True).stdout
    require(state==b'')


def _verify_publication_source(source_root,source,component_source,phase_source):
    captured=capture_source_tree(source_root,source)
    require(captured['result-validator']==component_source and captured['phase-validator']==phase_source)
    check_source_commit(source_root,source['commit'])


def _public_directory(output_directory,temporary_root):
    from pathlib import Path
    root,output=Path(temporary_root),Path(output_directory)
    require(root.is_absolute() and output.is_absolute())
    relative=output.relative_to(root)
    require(len(relative.parts)==2 and re.fullmatch('windows-reliability-token-[A-Za-z0-9-]{1,96}',relative.parts[0]) is not None and relative.parts[1]=='public')
    # Temporary-root aliases are platform-trusted; every selected output
    # descendant is plain and its identity is checked before/after publication.
    current=root.resolve(strict=True);chain=[(current,_identity(_plain_status(current,True)))]
    for part in relative.parts:
        current=current/part;chain.append((current,_identity(_plain_status(current,True))))
    return current,chain


def _remove_owned_link(path,identity,chain):
    import os
    if path is None or identity is None:return True
    try:
        if not os.path.lexists(path):return True
        _same_directories(chain)
        if _identity(_plain_status(path))==identity:os.unlink(path)
        # A foreign replacement is deliberately preserved, never followed.
        return True
    except Exception:
        return False


def publish_envelope(value,expected_source,expected_host,probe_data,component_source,phase_source,output_directory,temporary_root,source_root):
    """Exclusive fsynced staging and atomic no-overwrite link, with rollback.

    This primitive checks source files/Git, not its own executing-code origin;
    the fixed parent/publisher entrypoints must bind that separately.
    """
    import os
    import uuid
    stage=target=None;owned=None;chain=[];descriptor=None;failed=False;cleanup_failed=False;result=None;stage_created=False
    try:
        validate_envelope(value,expected_source,expected_host,probe_data,component_source,phase_source)
        _verify_publication_source(source_root,expected_source,component_source,phase_source)
        payload=encode_bounded_json(value)
        directory,chain=_public_directory(output_directory,temporary_root);target=directory/'receipt.json'
        require(not os.path.lexists(target))
        stage=directory/('.receipt-'+uuid.uuid4().hex+'.tmp')
        descriptor=os.open(stage,os.O_WRONLY|os.O_CREAT|os.O_EXCL|getattr(os,'O_BINARY',0)|getattr(os,'O_NOFOLLOW',0),0o600)
        stage_created=True
        opened=os.fstat(descriptor);owned=_identity(opened)
        require(opened.st_nlink==1 and _identity(_plain_status(stage))==owned)
        with os.fdopen(descriptor,'wb') as stream:
            descriptor=None
            require(stream.write(payload)==len(payload));stream.flush();os.fsync(stream.fileno())
            completed=os.fstat(stream.fileno())
            require(_identity(completed)==owned and completed.st_nlink==1 and completed.st_size==len(payload))
        _same_directories(chain);require(_identity(_plain_status(stage))==owned)
        _verify_publication_source(source_root,expected_source,component_source,phase_source)
        os.link(stage,target,follow_symlinks=False)
        linked=_plain_status(target);require(_identity(linked)==owned and linked.st_nlink==2)
        _same_directories(chain);stage_status=_plain_status(stage)
        require(_identity(stage_status)==owned and stage_status.st_nlink==2)
        os.unlink(stage)
        _same_directories(chain);final=_plain_status(target)
        require(_identity(final)==owned and final.st_nlink==1 and final.st_size==len(payload))
        descriptor=os.open(target,os.O_RDONLY|getattr(os,'O_BINARY',0)|getattr(os,'O_NOFOLLOW',0))
        with os.fdopen(descriptor,'rb') as stream:
            descriptor=None;opened=os.fstat(stream.fileno())
            require(_identity(opened)==owned and opened.st_nlink==1)
            actual=stream.read(65537);after=os.fstat(stream.fileno())
        require(_identity(after)==owned and after.st_nlink==1 and after.st_size==len(payload) and after.st_mtime_ns==opened.st_mtime_ns)
        require(actual==payload)
        decode_envelope(actual,expected_source,expected_host,probe_data,component_source,phase_source)
        _verify_publication_source(source_root,expected_source,component_source,phase_source)
        _same_directories(chain);last=_plain_status(target)
        require(_identity(last)==owned and last.st_nlink==1 and last.st_size==len(payload) and last.st_mtime_ns==after.st_mtime_ns)
        result={'sha256':hashlib.sha256(payload).hexdigest(),'bytes':len(payload)}
    except Exception:
        failed=True
    finally:
        if descriptor is not None:
            try:os.close(descriptor)
            except Exception:cleanup_failed=True
        # Creation succeeded but an initial identity failure leaves ownership
        # unproven. Preserve the path and report cleanup uncertainty.
        if stage_created and owned is None:cleanup_failed=True
        if not _remove_owned_link(stage,owned,chain):cleanup_failed=True
        if failed or cleanup_failed:
            if not _remove_owned_link(target,owned,chain):cleanup_failed=True
    if failed or cleanup_failed:raise TokenPublicationFailure(cleanup_failed) from None
    return result

def _private_packet_location(bundle_path,temporary_root):
    from pathlib import Path
    root,path=Path(temporary_root),Path(bundle_path)
    require(root.is_absolute() and path.is_absolute())
    relative=path.relative_to(root)
    require(len(relative.parts)==3 and re.fullmatch('windows-reliability-token-[A-Za-z0-9-]{1,96}',relative.parts[0]) is not None and relative.parts[1:]==('private','bundle.json'))
    current=root.resolve(strict=True);chain=[(current,_identity(_plain_status(current,True)))]
    for part in relative.parts[:-1]:
        current=current/part;chain.append((current,_identity(_plain_status(current,True))))
    return current/'bundle.json',chain


def finalize_token_bundle(bundle_path,temporary_root,output_directory,source_root,executed_validator_sha256):
    """Admit/remove a private packet before publication.

    The fixed captured-code bootstrap supplies its actual executed source hash.
    This library argument alone is not an executing-code provenance proof.
    """
    import os
    path=None;chain=[];owned=None;failed=False;cleanup_failed=False;value=None;code=None
    try:
        path,chain=_private_packet_location(bundle_path,temporary_root)
        before=_plain_status(path)
        data=_read_stable_regular(path,131072)
        require(_identity(_plain_status(path))==_identity(before));owned=_identity(before)
        _same_directories(chain)
        output,output_chain=_public_directory(output_directory,temporary_root)
        require(output.parent==path.parent.parent)
        packet=json.loads(data.decode('ascii'),object_pairs_hook=_unique,parse_constant=_constant)
        obj(packet,'source host query probe');validate_source(packet['source']);validate_host(packet['host'])
        digest(executed_validator_sha256)
        require(next(file['sha256'] for file in packet['source']['files'] if file['id']=='validator')==executed_validator_sha256)
        captured=capture_source_tree(source_root,packet['source']);check_source_commit(source_root,packet['source']['commit'])
        component_source=captured['result-validator'];phase_source=captured['phase-validator']
        value,code=decode_packet(data,packet['source'],packet['host'],component_source,phase_source)
        _same_directories(chain);_same_directories(output_chain)
    except Exception:
        failed=True
    finally:
        if owned is not None:
            try:
                _same_directories(chain)
                # A replacement is neither removed nor accepted as successful
                # private cleanup. No public artifact may precede this gate.
                require(_identity(_plain_status(path))==owned)
                os.unlink(path);require(not os.path.lexists(path))
                _same_directories(chain)
            except Exception:
                cleanup_failed=True
    if failed or cleanup_failed:raise TokenPublicationFailure(cleanup_failed) from None
    try:
        result=publish_envelope(value,packet['source'],packet['host'],packet['probe'].encode('ascii'),component_source,phase_source,output_directory,temporary_root,source_root)
        return result,code
    except TokenPublicationFailure:
        raise
    except Exception:
        raise TokenPublicationFailure() from None

def run_token_publisher(arguments,executed_validator_sha256):
    """Fixed captured-bootstrap entry; emit only a bounded publication result."""
    import sys
    cleanup_failed=False
    try:
        require(type(arguments) is list and len(arguments)==4 and all(type(argument) is str for argument in arguments))
        result,code=finalize_token_bundle(*arguments,executed_validator_sha256)
        obj(result,'sha256 bytes');digest(result['sha256']);integer(result['bytes'],65536);require(result['bytes']>0)
        require(type(code) is int and code in (0,1,2))
        text=json.dumps(result,sort_keys=True,separators=(',',':'));require(len(text)<=128)
        print(text)
        return code
    except TokenPublicationFailure as error:
        cleanup_failed=error.cleanup_failed
    except Exception:
        pass
    print('token-publication-failed cleanupFailed='+str(cleanup_failed).lower(),file=sys.stderr)
    return 1



def _admission_query(query,code,phase_validator,with_component):
    keys='started timedOut outputLimited rootExited readersSettled reaped exitCode elapsedMs phases hasFailure hasCleanupFailure hasReaderFailure stdoutChars stderrChars stdoutSha256 stderrSha256'
    obj(query,('kind stdoutDigestScope ' if with_component else '')+keys)
    if with_component:require(query['kind']=='effective-token-facts' and query['stdoutDigestScope']=='decoded-text-utf8')
    for key in ('started','rootExited','readersSettled','reaped'):require(query[key] is True)
    for key in ('timedOut','outputLimited','hasFailure','hasCleanupFailure','hasReaderFailure'):require(query[key] is False)
    integer(query['exitCode'],2);require(query['exitCode']==code)
    integer(query['elapsedMs'],0x7fffffffffffffff);integer(query['stdoutChars'],32768 if with_component else 129);require(query['stdoutChars']>0)
    integer(query['stderrChars'],0);digest(query['stdoutSha256']);require(query['stderrSha256']==hashlib.sha256(b'').hexdigest())
    phases=query['phases'];obj(phases,'unit frequency '+' '.join(PHASE_NAMES))
    require(phases['unit']=='stopwatch-elapsed-ticks');integer(phases['frequency'],0x7fffffffffffffff);require(phases['frequency']>0)
    for name in PHASE_NAMES:
        if phases[name] is not None:integer(phases[name],0x7fffffffffffffff)
    require(phase_validator(query))


def admit_token_artifact(output_directory,temporary_root,source_root,expected_source,execution):
    """Read-only artifact gate after parent/publisher admission.

    Revalidates public shape, component, phases, parent outcome, source and
    exact published bytes. The deleted private probe's digest is retained,
    not reconstructed or independently re-proven at this later boundary.
    """
    try:
        validate_source(expected_source)
        obj(execution,'kind captureAttempted metadataComplete sourceBound hostBound referencesReleased cleanupFailed failureStage query publisherQuery publication publicationSucceeded exitCode')
        require(execution['kind']=='coven.effective-token-execution.v1' and execution['captureAttempted'] is False and execution['metadataComplete'] is False)
        for key in ('sourceBound','hostBound','referencesReleased','publicationSucceeded'):require(execution[key] is True)
        require(execution['cleanupFailed'] is False and execution['failureStage']=='none');integer(execution['exitCode'],2)
        publication=execution['publication'];obj(publication,'sha256 bytes');digest(publication['sha256']);integer(publication['bytes'],65536);require(publication['bytes']>0)
        captured=capture_source_tree(source_root,expected_source);check_source_commit(source_root,expected_source['commit'])
        directory,chain=_public_directory(output_directory,temporary_root)
        data=_read_stable_regular(directory/'receipt.json',65536)
        require(len(data)==publication['bytes'] and hashlib.sha256(data).hexdigest()==publication['sha256'])
        value=_decode(data,65536);obj(value,'kind captureAttempted metadataComplete observationScope source host query component')
        require(value['kind']=='coven.effective-token-facts.v1' and value['captureAttempted'] is False and value['metadataComplete'] is False and value['observationScope']=='owned-child-effective-token')
        validate_source(value['source']);require(value['source']==expected_source);validate_host(value['host'])
        hashes={file['id']:file['sha256'] for file in expected_source['files']}
        decoder=_component_decoder(captured['result-validator'],hashes['result-validator'])
        component=decoder.load_component(encode_bounded_json(value['component'],4096));code=decoder.expected_exit(component)
        integer(code,2);require(code==execution['exitCode'])
        phase=_phase_validator(captured['phase-validator'],hashes['phase-validator'])
        _admission_query(value['query'],code,phase,True);_admission_query(execution['query'],code,phase,True)
        require(value['query']==execution['query'])
        _admission_query(execution['publisherQuery'],code,phase,False)
        # The fixed Python entry emits this one object with a platform newline.
        response=json.dumps(publication,sort_keys=True,separators=(',',':')).encode('ascii')
        observed=execution['publisherQuery']
        require(any(observed['stdoutChars']==len(response+newline) and observed['stdoutSha256']==hashlib.sha256(response+newline).hexdigest() for newline in (b'\n',b'\r\n')))
        require(capture_source_tree(source_root,expected_source)==captured);check_source_commit(source_root,expected_source['commit']);_same_directories(chain)
        final=_read_stable_regular(directory/'receipt.json',65536);require(final==data);_same_directories(chain)
        return publication.copy()
    except Exception:
        raise ValueError('Token artifact admission failed') from None

def run_token_artifact_admission(arguments,executed_validator_sha256):
    """Fixed captured-code workflow entry, separate from the private publisher."""
    import os
    import sys
    from pathlib import Path
    try:
        require(type(arguments) is list and len(arguments)==5 and all(type(item) is str for item in arguments))
        source_path,temporary_root,output_directory,source_root,snapshot_hash=arguments
        digest(snapshot_hash);digest(executed_validator_sha256)
        directory,chain=_public_directory(output_directory,temporary_root)
        candidate=Path(source_path);relative=candidate.relative_to(Path(temporary_root))
        require(len(relative.parts)==2 and relative.parts[1]=='source.json')
        candidate_directory,_=_public_directory(candidate.parent/'public',temporary_root);require(candidate_directory==directory)
        source_data=_read_stable_regular(directory.parent/'source.json',8192)
        require(hashlib.sha256(source_data).hexdigest()==snapshot_hash)
        source=_decode(source_data,8192);validate_source(source)
        require(next(file['sha256'] for file in source['files'] if file['id']=='validator')==executed_validator_sha256)
        execution_text=os.environ.get('EXECUTION_JSON','');execution_data=execution_text.encode('ascii',errors='strict')
        execution=_decode(execution_data,8192)
        require(Path(os.environ.get('PUBLIC_RECEIPT',''))==Path(output_directory)/'receipt.json')
        admit_token_artifact(output_directory,temporary_root,source_root,source,execution)
        _same_directories(chain)
        print('token-artifact-admission-pass')
        return 0
    except Exception:
        print('token-artifact-admission-failed',file=sys.stderr)
        return 1


if __name__=='__main__':
    import sys
    print('token-publisher-requires-captured-bootstrap',file=sys.stderr)
    raise SystemExit(1)
