"""Strict public projection validation; never include rejected values in errors."""
from datetime import datetime, timedelta
import re

TOOLS = {'xperf', 'tracelog', 'tracerpt', 'WPAExporter', 'wevtutil', 'whoami'}
QUERIES = {'xperf-start-help', 'xperf-providers-help', 'xperf-loggers', 'xperf-providers', 'xperf-profint', 'xperf-processing-help', 'xperf-dumper-help', 'xperf-tracestats-help', 'xperf-cswitch-help', 'kernel-file-schema', 'kernel-file-schema-no-messages', 'token-groups', 'token-privileges'}
SCHEMAS = {'kernel-file-schema', 'kernel-file-schema-no-messages'}
FILES = {'run-preflight.ps1', 'query-helper.ps1', 'query-plan.ps1', 'query-summary.ps1', 'query-executor.ps1', 'tool-identity.ps1', 'tool-discovery.ps1', 'provider-schema.ps1', 'preflight-core.ps1', 'receipt-writer.ps1', 'source-stability.ps1'}
PENDING = {'provider-schema-decoding', 'token-summary', 'controller-capability-decoding', 'outer-workflow-guard', 'native-validation'}
NAMES = {'Irp', 'IrpPtr', 'FileObject', 'FileKey', 'IssuingThreadId', 'ThreadId', 'Status', 'ExtraInfo', 'InfoClass', 'FileName', 'FilePath', 'CreateOptions', 'CreateAttributes', 'ShareAccess', 'IoSize', 'IoFlags', 'ByteOffset', 'TTID'}
TYPES = {'win:Pointer', 'win:UInt8', 'win:UInt16', 'win:UInt32', 'win:UInt64', 'win:Int8', 'win:Int16', 'win:Int32', 'win:Int64', 'win:UnicodeString', 'win:AnsiString', 'win:HexInt32', 'win:HexInt64', 'win:Binary'}


def boolean(v):
    return type(v) is bool


def unsigned(v):
    return type(v) is int and 0 <= v <= 2**63 - 1


def digest(v):
    return type(v) is str and re.fullmatch('[0-9a-f]{64}', v) is not None


def member(values):
    return lambda v: type(v) is str and v in values


def nullable(predicate):
    return lambda v: v is None or predicate(v)


def matches(value, spec):
    if type(spec) is dict:
        return type(value) is dict and value.keys() == spec.keys() and all(matches(value[k], s) for k, s in spec.items())
    if type(spec) is list:
        return type(value) is list and len(value) <= 1024 and all(matches(v, spec[0]) for v in value)
    if callable(spec):
        try:
            return bool(spec(value))
        except (TypeError, ValueError, KeyError, OverflowError):
            return False
    return type(value) is type(spec) and value == spec


def utc(v):
    if type(v) is not str or len(v) > 40:
        return False
    try:
        return datetime.fromisoformat(v.replace('Z', '+00:00')).utcoffset() == timedelta(0)
    except ValueError:
        return False


TOOL = dict(tool=member(TOOLS), locationKind=member({'path', 'wpt', 'system32'}), present=boolean, identityFailed=boolean,
            sha256=nullable(digest), fileVersion=nullable(lambda v: type(v) is list and len(v) == 4 and all(type(n) is int and 0 <= n <= 65535 for n in v)),
            signatureStatus=member({'unavailable', 'Valid', 'UnknownError', 'NotSigned', 'HashMismatch', 'NotTrusted', 'NotSupportedFileFormat', 'Incompatible', 'query-failed'}),
            microsoftSigner=boolean, originalFilenameMatched=boolean, admitted=boolean)
QUERY = dict(id=member(QUERIES), started=boolean, timedOut=boolean, outputLimited=boolean, rootExited=boolean, readersSettled=boolean, reaped=boolean,
             exitCode=nullable(lambda v: type(v) is int and -(2**31) <= v < 2**31), elapsedMs=unsigned, hasFailure=boolean, hasCleanupFailure=boolean, hasReaderFailure=boolean,
             stdoutChars=lambda v: unsigned(v) and v <= 524288, stderrChars=lambda v: unsigned(v) and v <= 524288,
             stdoutSha256=digest, stderrSha256=digest, available=True, executed=True)
SKIPPED = dict(id=member(QUERIES), available=boolean, executed=False, skippedAfterCleanupFailure=boolean)
THREW = dict(id=member(QUERIES), available=True, executed=True, invocationThrew=True, cleanupProven=False)
FIELD = dict(name=nullable(member(NAMES)), nameSha256=digest, inType=nullable(member(TYPES)), typeSha256=digest)
EVENT = dict(id=lambda v: unsigned(v) and v <= 65535, version=lambda v: unsigned(v) and v <= 255, fields=[FIELD], templateFieldsFound=boolean)
SCHEMA = dict(providerMatched=boolean, parseFailed=boolean, events=[EVENT], unsupportedEvents=unsigned, schemaComplete=False)
PROVIDER = lambda v: matches(v, dict(id=member(SCHEMAS), parsed=False)) or matches(v, dict(id=member(SCHEMAS), parsed=True, schema=SCHEMA))
PUBLIC = dict(formatVersion=1, captureAttempted=False, metadataComplete=False, pending=[member(PENDING)], tools=[TOOL],
              queries=[lambda v: any(matches(v, s) for s in (QUERY, SKIPPED, THREW))], providerSchemas=[PROVIDER], aborted=boolean,
              clock=dict(isHighResolution=boolean, frequency=lambda v: unsigned(v) and v > 0, before=unsigned, utc=utc, after=unsigned),
              sourceFiles=[dict(file=member(FILES), sha256=digest)], sourceStable=boolean)


def validate_public_receipt(value):
    good = matches(value, PUBLIC)
    if good:
        for key, field, expected in [('tools', 'tool', TOOLS), ('queries', 'id', QUERIES), ('providerSchemas', 'id', SCHEMAS), ('sourceFiles', 'file', FILES)]:
            good = good and len(value[key]) == len(expected) and {v[field] for v in value[key]} == expected
        good = good and len(value['pending']) == len(PENDING) and set(value['pending']) == PENDING
        good = good and value['clock']['after'] >= value['clock']['before']
        good = good and all(not t['admitted'] or (t['present'] and not t['identityFailed'] and t['signatureStatus'] == 'Valid' and t['microsoftSigner'] and t['originalFilenameMatched'] and t['sha256'] is not None and t['fileVersion'] is not None) for t in value['tools'])
    if not good:
        raise ValueError('Invalid public preflight receipt')


def parse_public_receipt(data):
    """Reject ambiguous encodings before validating the complete public shape."""
    import json

    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError('Invalid public preflight receipt')
            result[key] = value
        return result

    def reject_constant(_value):
        raise ValueError('Invalid public preflight receipt')

    if type(data) is not bytes or len(data) > 8 * 1024 * 1024:
        raise ValueError('Invalid public preflight receipt')
    try:
        value = json.loads(data.decode('utf-8'), object_pairs_hook=unique_object, parse_constant=reject_constant)
        validate_public_receipt(value)
        return value
    except (ValueError, RecursionError):
        raise ValueError('Invalid public preflight receipt') from None


def require_junction_inspection():
    from pathlib import Path
    # Python 3.12+ supplies this API. Refuse older/missing-capability hosts rather
    # than silently dropping Windows directory-redirection checks.
    if not callable(getattr(Path, 'is_junction', None)):
        raise ValueError('Public preflight receipt requires junction inspection')


def read_public_receipt(source, temporary_root):
    """Read once from the owned public location; return only validated values."""
    import os
    from pathlib import Path
    import stat

    descriptor = None
    try:
        require_junction_inspection()
        source, temporary_root = Path(source), Path(temporary_root)
        if not source.is_absolute() or not temporary_root.is_absolute():
            raise ValueError()
        relative = source.relative_to(temporary_root)
        if len(relative.parts) != 3 or not relative.parts[0].startswith('coven-native-preflight-') or relative.parts[1:] != ('public', 'receipt.json'):
            raise ValueError()
        current = temporary_root.resolve(strict=True)
        for part in relative.parts:
            current = current / part
            if current.is_symlink() or current.is_junction():
                raise ValueError()
        if not (current.parent.parent / 'private').is_dir():
            raise ValueError()
        before = current.stat(follow_symlinks=False)
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_size > 8 * 1024 * 1024:
            raise ValueError()
        descriptor = os.open(current, os.O_RDONLY | getattr(os, 'O_BINARY', 0) | getattr(os, 'O_NOFOLLOW', 0))
        opened = os.fstat(descriptor)
        if (opened.st_dev, opened.st_ino, opened.st_nlink) != (before.st_dev, before.st_ino, 1):
            raise ValueError()
        with os.fdopen(descriptor, 'rb') as stream:
            descriptor = None
            data = stream.read(8 * 1024 * 1024 + 1)
            after = os.fstat(stream.fileno())
        if (after.st_size, after.st_mtime_ns) != (opened.st_size, opened.st_mtime_ns):
            raise ValueError()
        # Callers publish this parsed value by re-serializing it. Never re-open or
        # copy original bytes after validation; a path is not an immutable snapshot.
        return parse_public_receipt(data)
    except (OSError, ValueError, RuntimeError):
        raise ValueError('Invalid public preflight receipt') from None
    finally:
        if descriptor is not None:
            os.close(descriptor)


def publish_public_receipt(source, temporary_root, output_directory):
    """Publish a newly serialized validated snapshot into the owned public directory."""
    import hashlib
    import json
    from pathlib import Path

    target = None
    created = False
    try:
        require_junction_inspection()
        root, output = Path(temporary_root), Path(output_directory)
        if not root.is_absolute() or not output.is_absolute():
            raise ValueError()
        relative = output.relative_to(root)
        if len(relative.parts) != 2 or not relative.parts[0].startswith('windows-reliability-metadata-') or relative.parts[1] != 'public':
            raise ValueError()
        current = root.resolve(strict=True)
        for part in relative.parts:
            current = current / part
            if current.is_symlink() or current.is_junction():
                raise ValueError()
        if not current.is_dir():
            raise ValueError()
        value = read_public_receipt(source, temporary_root)
        payload = json.dumps(value, ensure_ascii=True, allow_nan=False, sort_keys=True, separators=(',', ':')).encode('utf-8') + b'\n'
        target = current / 'receipt.json'
        with target.open('xb') as stream:
            created = True
            stream.write(payload)
        return {'sha256': hashlib.sha256(payload).hexdigest(), 'bytes': len(payload)}
    except (OSError, ValueError, RuntimeError):
        if created and target is not None:
            target.unlink(missing_ok=True)
        raise ValueError('Public preflight receipt publication failed') from None


if __name__ == '__main__':
    import json
    import sys

    try:
        if len(sys.argv) != 4:
            raise ValueError()
        result = publish_public_receipt(*sys.argv[1:])
    except Exception:
        print('Public preflight receipt publication failed', file=sys.stderr)
        raise SystemExit(1) from None
    print(json.dumps(result, sort_keys=True))
