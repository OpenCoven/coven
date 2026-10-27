"""Strict bounded token-result component decoding, without native operations.

These are owned-child observations, not recording permission or causal evidence.
Whole source/host/query admission and publication are separate boundaries.
"""
import json

MAX_COMPONENT_BYTES=4096
RESULT_KEYS='failure nativeError cleanupFailed clearAttempts clearFailures freeAttempts freeFailures closeAttempts closeFailures'


def require(ok):
    if not ok:raise ValueError('Invalid bounded token result')


def obj(value,keys):
    require(type(value) is dict and set(value)==set(keys.split()))


def integer(value,maximum):
    require(type(value) is int and 0<=value<=maximum)


def boolean(value):
    require(type(value) is bool)


def _result(value,with_facts):
    obj(value,RESULT_KEYS+(' facts' if with_facts else ''))
    integer(value['failure'],10);integer(value['nativeError'],0xffffffff)
    boolean(value['cleanupFailed'])
    failed=False
    for operation,maximum in [('clear',6),('free',6),('close',2)]:
        attempted=value[operation+'Attempts'];failures=value[operation+'Failures']
        integer(attempted,maximum);integer(failures,maximum)
        require(failures<=attempted);failed=failed or failures>0
    require(value['clearAttempts']==value['freeAttempts'])
    require(value['cleanupFailed']==failed)
    failure=value['failure']
    if failure not in {1,2,4,5,7}:require(value['nativeError']==0)
    if failure==0:
        require(value['clearAttempts']==6 and value['closeAttempts']==2)
    if not with_facts:return
    facts=value['facts']
    if failure!=0 or failed:
        require(facts is None);return
    obj(facts,'context tokenType impersonationLevel elevationType elevated endpointsMatch privilege')
    integer(facts['context'],1);integer(facts['tokenType'],2)
    if facts['context']==0:
        require(facts['tokenType']==2);integer(facts['impersonationLevel'],3)
    else:require(facts['tokenType']==1 and facts['impersonationLevel'] is None)
    integer(facts['elevationType'],3);require(facts['elevationType']>=1)
    boolean(facts['elevated']);require(facts['endpointsMatch'] is True)
    privilege=facts['privilege'];obj(privilege,'present enabled enabledByDefault removed usedForAccess')
    for flag in privilege.values():boolean(flag)
    if not privilege['present']:require(not any(privilege[key] for key in privilege if key!='present'))


def validate_component(value):
    require(type(value) is dict)
    kind=value.get('kind')
    if kind=='coven.effective-token-result.v1':
        obj(value,'kind captureAttempted privilegeName result')
        require(value['captureAttempted'] is False and value['privilegeName']=='SeSystemProfilePrivilege')
        _result(value['result'],True)
    elif kind=='coven.effective-token-export-failure.v1':
        obj(value,'kind captureAttempted factsWithheld exportError resultSummary')
        require(value['captureAttempted'] is False and value['factsWithheld'] is True)
        require(type(value['exportError']) is str and value['exportError'] in {'output-limit','serialization-failed'})
        _result(value['resultSummary'],False)
    else:require(False)
    return value


def _unique(pairs):
    value={}
    for key,item in pairs:
        require(key not in value);value[key]=item
    return value


def _constant(_):
    require(False)


def load_component(data):
    try:
        require(type(data) is bytes and 0<len(data)<=MAX_COMPONENT_BYTES)
        value=json.loads(data.decode('utf-8',errors='strict'),object_pairs_hook=_unique,parse_constant=_constant)
        return validate_component(value)
    except (ValueError,TypeError,UnicodeError,RecursionError):
        raise ValueError('Invalid bounded token result') from None


def expected_exit(value):
    validate_component(value)
    if value['kind']=='coven.effective-token-export-failure.v1':return 2
    result=value['result']
    return 0 if result['failure']==0 and not result['cleanupFailed'] else 1
