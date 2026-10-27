#nullable enable
using System;
using System.Buffers.Binary;
using System.Collections.Generic;
using System.Linq;
using System.Text.Json;

public sealed class CovenTokenReaderControlReport {
    public int Cases { get; }
    public string[] Failures { get; }
    public CovenTokenReaderControlReport(int cases,string[] failures) { Cases=cases;Failures=failures; }
}
internal sealed class ControlledTokenApi : ICovenTokenQueryApi {
    public readonly List<string> Calls=new();
    public readonly Dictionary<IntPtr,byte[]> Buffers=new();
    public readonly List<byte[]> Copies=new();
    public readonly Dictionary<string,uint> Errors=new();
    public readonly HashSet<string> Throws=new();
    public readonly HashSet<string> False=new();
    public bool EndpointThreadAbsent,ForcePrimaryThread,ForceImpersonationProcess,EndpointPrimaryThread,BadImpersonationLevel,UnownedFailureHandle,ShortCopy;
    public readonly List<IntPtr> Closed=new();
    private readonly HashSet<IntPtr> processTokens=new();
    public bool ThreadAbsent,DuplicateHandle,ChangedEndpoint,ChangedModified,UnknownType,UnknownElevation;
    public bool ShortFixed,ChangedSize,UnexpectedProbeSuccess,InvalidHandle,UnknownFlags;
    public uint ProbeBytes=16,Elevated=2;
    private int opens,statistics,nextBuffer=100;
    private void Call(string name) { Calls.Add(name);if(Throws.Contains(name))throw new InvalidOperationException("PRIVATE_EXCEPTION_SENTINEL"); }
    private bool Ok(string name,out uint error) { error=Errors.TryGetValue(name,out var e)?e:5;return !Errors.ContainsKey(name); }
    public bool OpenCurrentThreadToken(out IntPtr handle,out uint error) {
        Call("open-thread");handle=IntPtr.Zero;
        if(ThreadAbsent||(EndpointThreadAbsent&&opens>0)){error=1008;return false;}
        if(!Ok("open-thread",out error)){if(UnownedFailureHandle)handle=new IntPtr(999);return false;}
        handle=new IntPtr(InvalidHandle?-2:(DuplicateHandle?10:10+opens++));return true;
    }
    public bool OpenCurrentProcessToken(out IntPtr handle,out uint error) {
        Call("open-process");handle=IntPtr.Zero;if(!Ok("open-process",out error))return false;
        handle=new IntPtr(DuplicateHandle?10:10+opens++);processTokens.Add(handle);return true;
    }
    public bool LookupSelectedPrivilege(out uint low,out int high,out uint error) {
        Call("lookup");low=7;high=-9;return Ok("lookup",out error);
    }
    public IntPtr Allocate(int bytes) { Call("allocate");var p=new IntPtr(nextBuffer++);Buffers.Add(p,new byte[bytes]);return p; }
    public bool Query(IntPtr token,int info,IntPtr buffer,int bytes,out int returned,out uint error) {
        string name=buffer==IntPtr.Zero?"probe":$"query-{info}";Call(name);
        returned=bytes;error=0;
        if(buffer==IntPtr.Zero){returned=(int)ProbeBytes;error=Errors.TryGetValue("probe",out var probeError)?probeError:122;return UnexpectedProbeSuccess;}
        if(!Ok(name,out error))return false;
        var data=Buffers[buffer];
        if(info==10){
            statistics++;
            BinaryPrimitives.WriteUInt64LittleEndian(data.AsSpan(0,8),(ChangedEndpoint&&statistics==3)?2ul:1ul);
            BinaryPrimitives.WriteUInt32LittleEndian(data.AsSpan(24,4),UnknownType?7u:(processTokens.Contains(token)?(ForceImpersonationProcess?2u:1u):((ForcePrimaryThread||(EndpointPrimaryThread&&statistics==3))?1u:2u)));
            // Primary tokens do not give the impersonation-level word semantic meaning.
            BinaryPrimitives.WriteUInt32LittleEndian(data.AsSpan(28,4),!processTokens.Contains(token)||ForceImpersonationProcess?(BadImpersonationLevel?4u:2u):uint.MaxValue);
            BinaryPrimitives.WriteUInt64LittleEndian(data.AsSpan(48,8),(ChangedModified&&statistics==2)?2ul:1ul);
        } else if(info==18) BinaryPrimitives.WriteUInt32LittleEndian(data,UnknownElevation?0u:3u);
        else if(info==20) BinaryPrimitives.WriteUInt32LittleEndian(data,Elevated);
        else if(info==3){
            BinaryPrimitives.WriteUInt32LittleEndian(data,1);
            BinaryPrimitives.WriteUInt32LittleEndian(data.AsSpan(4,4),7);
            BinaryPrimitives.WriteInt32LittleEndian(data.AsSpan(8,4),-9);
            BinaryPrimitives.WriteUInt32LittleEndian(data.AsSpan(12,4),UnknownFlags?8u:1u);
            if(ChangedSize)returned=bytes-1;
        }
        if(ShortFixed&&info==10)returned=55;
        return true;
    }
    public byte[] Copy(IntPtr buffer,int bytes) { Call("copy");var result=ShortCopy?new byte[1]:(byte[])Buffers[buffer].Clone();Copies.Add(result);return result; }
    public bool Clear(IntPtr buffer,int bytes) { Call("clear");Array.Clear(Buffers[buffer]);return !False.Contains("clear"); }
    public bool Free(IntPtr buffer) { Call("free");Buffers.Remove(buffer);return !False.Contains("free"); }
    public bool Close(IntPtr token) { Closed.Add(token);Call("close");return !False.Contains("close"); }
}
public static class CovenTokenReaderControls {
    private static void Require(bool value) { if(!value)throw new InvalidOperationException("Controlled assertion failed"); }
    private static CovenTokenQueryResult Run(ControlledTokenApi api) => CovenTokenQueryReader.Collect(api);
    public static CovenTokenReaderControlReport Run() {
        int cases=0;var failures=new List<string>();
        void Case(string name,Action action){cases++;try{action();}catch{failures.Add(name);}}
        Case("owned-thread-success-nonzero-elevation",()=> {
            var api=new ControlledTokenApi();var r=Run(api);
            Require(r.Failure==CovenTokenFailure.None&&!r.CleanupFailed&&r.Facts is not null);
            Require(r.Facts!.Context==CovenTokenContext.Thread&&r.Facts.Elevated&&r.Facts.ElevationType==3&&r.Facts.EndpointsMatch);
            Require(r.Facts.Privilege.Present&&!r.Facts.Privilege.Enabled&&r.Facts.Privilege.EnabledByDefault);
            Require(api.Calls.SequenceEqual(new[]{"open-thread","lookup","allocate","query-10","copy","clear","free","allocate","query-18","copy","clear","free","allocate","query-20","copy","clear","free","probe","allocate","query-3","copy","clear","free","allocate","query-10","copy","clear","free","open-thread","allocate","query-10","copy","clear","free","close","close"}));
            Require(api.Copies.All(b=>b.All(x=>x==0))&&api.Buffers.Count==0);
            Require(api.Closed.SequenceEqual(new[]{new IntPtr(11),new IntPtr(10)}));
            string wire=JsonSerializer.Serialize(r);Require(!wire.Contains("PRIVATE_")&&!wire.Contains("TokenId")&&!wire.Contains("ModifiedId")&&!wire.Contains("Authentication"));
        });
        Case("explicit-no-token-process-fallback",()=> {var a=new ControlledTokenApi{ThreadAbsent=true};var r=Run(a);Require(r.Facts?.Context==CovenTokenContext.Process&&a.Calls.Count(x=>x=="open-process")==2);});
        foreach(uint error in new[]{5u,1347u,999u})Case("no-fallback-"+error,()=>{var a=new ControlledTokenApi();a.Errors["open-thread"]=error;var r=Run(a);Require(r.Failure==CovenTokenFailure.OpenThread&&r.NativeError==error&&r.Facts is null&&!a.Calls.Contains("open-process")&&!a.Calls.Contains("close"));});
        Case("thread-primary-refused",()=>{var a=new ControlledTokenApi{ForcePrimaryThread=true};Require(Run(a).Failure==CovenTokenFailure.Layout&&!a.Calls.Contains("query-18")&&a.Calls.Count(x=>x=="query-10")==1);});
        Case("process-impersonation-refused",()=>{var a=new ControlledTokenApi{ThreadAbsent=true,ForceImpersonationProcess=true};Require(Run(a).Failure==CovenTokenFailure.Layout&&!a.Calls.Contains("query-18")&&a.Calls.Count(x=>x=="query-10")==1);});
        Case("endpoint-thread-primary-refused",()=>Require(Run(new ControlledTokenApi{EndpointPrimaryThread=true}).Failure==CovenTokenFailure.Layout));
        Case("process-primary-unused-level",()=>{var r=Run(new ControlledTokenApi{ThreadAbsent=true});Require(r.Facts?.TokenType==1&&r.Facts.ImpersonationLevel is null);});
        Case("impersonation-level-projected",()=>{var r=Run(new ControlledTokenApi());Require(r.Facts?.TokenType==2&&r.Facts.ImpersonationLevel==2);});
        Case("invalid-impersonation-level",()=>Require(Run(new ControlledTokenApi{BadImpersonationLevel=true}).Failure==CovenTokenFailure.Layout));
        Case("context-endpoint-changed",()=>{var r=Run(new ControlledTokenApi{EndpointThreadAbsent=true});Require(r.Failure==CovenTokenFailure.ChangedContext&&r.Facts is null);});
        Case("false-open-handle-unowned",()=>{var a=new ControlledTokenApi{UnownedFailureHandle=true};a.Errors["open-thread"]=5;var r=Run(a);Require(r.Facts is null&&a.Closed.Count==0);});
        Case("probe-error-preserved",()=>{var a=new ControlledTokenApi();a.Errors["probe"]=5;var r=Run(a);Require(r.Failure==CovenTokenFailure.Probe&&r.NativeError==5&&a.Calls.Count(x=>x=="allocate")==3);});
        Case("short-owned-copy-cleared",()=>{var a=new ControlledTokenApi{ShortCopy=true};var r=Run(a);Require(r.Failure==CovenTokenFailure.Size&&a.Copies.All(b=>b.All(x=>x==0))&&a.Buffers.Count==0);});
        Case("process-open-failure",()=>{var a=new ControlledTokenApi{ThreadAbsent=true};a.Errors["open-process"]=5;var r=Run(a);Require(r.Failure==CovenTokenFailure.OpenProcess&&r.Facts is null&&!a.Calls.Contains("close"));});
        Case("invalid-handle-never-closed",()=>{var a=new ControlledTokenApi{InvalidHandle=true};var r=Run(a);Require(r.Failure==CovenTokenFailure.Handle&&!a.Calls.Contains("close"));});
        Case("duplicate-owned-handle-closed-once",()=>{var a=new ControlledTokenApi{DuplicateHandle=true};var r=Run(a);Require(r.Failure==CovenTokenFailure.Handle&&a.Calls.Count(x=>x=="close")==1);});
        Case("lookup-failure-preserved",()=>{var a=new ControlledTokenApi();a.Errors["lookup"]=1313;var r=Run(a);Require(r.Failure==CovenTokenFailure.Lookup&&r.NativeError==1313&&r.Facts is null&&a.Calls.Last()=="close");});
        Case("endpoint-token-changed",()=>{var r=Run(new ControlledTokenApi{ChangedEndpoint=true});Require(r.Failure==CovenTokenFailure.ChangedContext&&r.Facts is null);});
        Case("modified-id-changed",()=>{var r=Run(new ControlledTokenApi{ChangedModified=true});Require(r.Failure==CovenTokenFailure.ChangedContext&&r.Facts is null);});
        Case("unknown-token-type",()=>Require(Run(new ControlledTokenApi{UnknownType=true}).Failure==CovenTokenFailure.Layout));
        Case("unknown-elevation-type",()=>Require(Run(new ControlledTokenApi{UnknownElevation=true}).Failure==CovenTokenFailure.Layout));
        Case("fixed-return-short",()=>Require(Run(new ControlledTokenApi{ShortFixed=true}).Failure==CovenTokenFailure.Size));
        Case("changed-variable-size-not-retried",()=>{var a=new ControlledTokenApi{ChangedSize=true};var r=Run(a);Require(r.Failure==CovenTokenFailure.Size&&a.Calls.Count(x=>x=="query-3")==1);});
        Case("unexpected-probe-success",()=>Require(Run(new ControlledTokenApi{UnexpectedProbeSuccess=true}).Failure==CovenTokenFailure.Probe));
        foreach(uint bytes in new[]{0u,3u,4097u})Case("probe-bounds-"+bytes,()=>{var a=new ControlledTokenApi{ProbeBytes=bytes};var r=Run(a);Require(r.Failure==CovenTokenFailure.Size&&a.Calls.Count(x=>x=="allocate")==3);});
        Case("unknown-selected-privilege-bits",()=>Require(Run(new ControlledTokenApi{UnknownFlags=true}).Failure==CovenTokenFailure.Layout));
        foreach(string stage in new[]{"query-10","query-18","query-20","query-3"})Case("read-failure-"+stage,()=>{var a=new ControlledTokenApi();a.Errors[stage]=6;var r=Run(a);Require(r.Failure==CovenTokenFailure.Read&&r.NativeError==6&&r.Facts is null&&a.Buffers.Count==0&&a.Copies.All(b=>b.All(x=>x==0)));});
        foreach(string stage in new[]{"allocate","copy","lookup"})Case("exception-"+stage,()=>{var a=new ControlledTokenApi();a.Throws.Add(stage);var r=Run(a);Require(r.Failure==CovenTokenFailure.Unexpected&&r.Facts is null&&a.Calls.Last()=="close");});
        foreach(string stage in new[]{"clear","free","close"})Case("false-cleanup-"+stage,()=>{var a=new ControlledTokenApi();a.False.Add(stage);var r=Run(a);Require(r.Failure==CovenTokenFailure.None&&r.CleanupFailed&&r.Facts is null&&a.Calls.Count(x=>x=="close")==2&&a.Calls.Count(x=>x=="free")==6);});
        Case("independent-all-cleanup-exceptions",()=>{var a=new ControlledTokenApi();foreach(var s in new[]{"clear","free","close"})a.Throws.Add(s);var r=Run(a);Require(r.Failure==CovenTokenFailure.None&&r.CleanupFailed&&r.Facts is null&&a.Calls.Count(x=>x=="close")==2&&a.Calls.Count(x=>x=="free")==6);});
        Case("primary-and-cleanup-failure",()=>{var a=new ControlledTokenApi();a.Errors["query-18"]=6;foreach(var s in new[]{"clear","free","close"})a.Throws.Add(s);var r=Run(a);Require(r.Failure==CovenTokenFailure.Read&&r.NativeError==6&&r.CleanupFailed&&r.Facts is null&&a.Calls.Count(x=>x=="free")==2&&a.Calls.Count(x=>x=="close")==1);});
        return new CovenTokenReaderControlReport(cases,failures.ToArray());
    }
}
