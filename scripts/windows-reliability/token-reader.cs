#nullable enable
using System;
using System.Buffers.Binary;
using System.Collections.Generic;

public enum CovenTokenFailure { None,OpenThread,OpenProcess,Handle,Lookup,Probe,Size,Read,Layout,ChangedContext,Unexpected }
public enum CovenTokenContext { Thread,Process }
public sealed class CovenEffectiveTokenFacts {
    public CovenTokenContext Context { get; }
    public int TokenType { get; }
    public int? ImpersonationLevel { get; }
    public int ElevationType { get; }
    public bool Elevated { get; }
    // Endpoint equality is not an atomic or continuously stable snapshot.
    public bool EndpointsMatch => true;
    public CovenTokenPrivilegeFacts Privilege { get; }
    internal CovenEffectiveTokenFacts(CovenTokenContext context,int type,int? level,int elevationType,bool elevated,CovenTokenPrivilegeFacts privilege) {
        Context=context;TokenType=type;ImpersonationLevel=level;ElevationType=elevationType;Elevated=elevated;Privilege=privilege;
    }
}
public sealed class CovenTokenQueryResult {
    public CovenTokenFailure Failure { get; }
    public uint NativeError { get; }
    public bool CleanupFailed => ClearFailures+FreeFailures+CloseFailures!=0;
    public int ClearAttempts { get; }
    public int ClearFailures { get; }
    public int FreeAttempts { get; }
    public int FreeFailures { get; }
    public int CloseAttempts { get; }
    public int CloseFailures { get; }
    public CovenEffectiveTokenFacts? Facts { get; }
    internal CovenTokenQueryResult(CovenTokenFailure failure,uint error,int clearAttempts,int clearFailures,int freeAttempts,int freeFailures,int closeAttempts,int closeFailures,CovenEffectiveTokenFacts? facts) {
        Failure=failure;NativeError=error;ClearAttempts=clearAttempts;ClearFailures=clearFailures;FreeAttempts=freeAttempts;FreeFailures=freeFailures;CloseAttempts=closeAttempts;CloseFailures=closeFailures;
        Facts=failure==CovenTokenFailure.None&&!CleanupFailed?facts:null;
    }
}
// Internal seam for deterministic controls. The native adapter is a separate boundary.
// It must select only this OS thread/process, request TOKEN_QUERY, and capture
// last-error immediately. No user callback/await/work transfer is permitted.
internal interface ICovenTokenQueryApi {
    bool OpenCurrentThreadToken(out IntPtr token,out uint error);
    bool OpenCurrentProcessToken(out IntPtr token,out uint error);
    bool LookupSelectedPrivilege(out uint low,out int high,out uint error);
    IntPtr Allocate(int bytes);
    bool Query(IntPtr token,int infoClass,IntPtr buffer,int bytes,out int returned,out uint error);
    byte[] Copy(IntPtr buffer,int bytes);
    bool Clear(IntPtr buffer,int bytes);
    bool Free(IntPtr buffer);
    bool Close(IntPtr token);
}
internal sealed class CovenTokenReadFailure : Exception {
    internal readonly CovenTokenFailure Failure;
    internal readonly uint NativeError;
    internal CovenTokenReadFailure(CovenTokenFailure failure,uint error=0) { Failure=failure;NativeError=error; }
}
internal sealed class CovenTokenQueryReader {
    private readonly ICovenTokenQueryApi api;
    private readonly List<IntPtr> handles=new();
    private int requested,clearAttempts,clearFailures,freeAttempts,freeFailures,closeAttempts,closeFailures;
    private CovenTokenQueryReader(ICovenTokenQueryApi api) { this.api=api; }
    internal static CovenTokenQueryResult Collect(ICovenTokenQueryApi api) => new CovenTokenQueryReader(api).Collect();
    private static bool Invalid(IntPtr value) => value==IntPtr.Zero||value==new IntPtr(-1)||value==new IntPtr(-2);
    private IntPtr Open(out CovenTokenContext context) {
        context=CovenTokenContext.Thread;
        bool opened=api.OpenCurrentThreadToken(out IntPtr token,out uint error);
        if(!opened) {
            if(error!=1008)throw new CovenTokenReadFailure(CovenTokenFailure.OpenThread,error);
            context=CovenTokenContext.Process;
            if(!api.OpenCurrentProcessToken(out token,out error))throw new CovenTokenReadFailure(CovenTokenFailure.OpenProcess,error);
        }
        // Only a successful, non-pseudo, distinct token open grants ownership.
        // A repeated handle is ambiguous: retain the original ownership once.
        if(Invalid(token)||handles.Contains(token))throw new CovenTokenReadFailure(CovenTokenFailure.Handle);
        handles.Add(token);return token;
    }
    private void Size(int bytes) {
        if(bytes<4||bytes>4096||requested>16384-bytes)throw new CovenTokenReadFailure(CovenTokenFailure.Size);
        requested=checked(requested+bytes);
    }
    private T Read<T>(IntPtr token,int info,int bytes,Func<byte[],T> project) {
        Size(bytes);
        IntPtr buffer=IntPtr.Zero;
        byte[]? copy=null;
        try {
            buffer=api.Allocate(bytes);
            if(Invalid(buffer))throw new CovenTokenReadFailure(CovenTokenFailure.Size);
            if(!api.Query(token,info,buffer,bytes,out int returned,out uint error))throw new CovenTokenReadFailure(CovenTokenFailure.Read,error);
            // Fixed sizes and the one probed variable size must remain exact.
            if(returned!=bytes)throw new CovenTokenReadFailure(CovenTokenFailure.Size);
            copy=api.Copy(buffer,bytes);
            if(copy is null||copy.Length!=bytes)throw new CovenTokenReadFailure(CovenTokenFailure.Size);
            try { return project(copy); }
            catch(CovenTokenReadFailure){throw;}
            catch(ArgumentException){throw new CovenTokenReadFailure(CovenTokenFailure.Layout);}
            catch(InvalidOperationException){throw new CovenTokenReadFailure(CovenTokenFailure.Layout);}
            catch(NotSupportedException){throw new CovenTokenReadFailure(CovenTokenFailure.Layout);}
        } finally {
            if(copy is not null)Array.Clear(copy);
            if(!Invalid(buffer)) {
                clearAttempts++;
                try{if(!api.Clear(buffer,bytes))clearFailures++;}catch{clearFailures++;}
                freeAttempts++;
                try{if(!api.Free(buffer))freeFailures++;}catch{freeFailures++;}
            }
        }
    }
    private readonly struct Statistics {
        internal readonly ulong Identity,Modified;
        internal readonly int Type;
        internal readonly int? Level;
        internal Statistics(byte[] bytes) {
            // TOKEN_STATISTICS is 56 bytes. Layout must also be proved by the
            // eventual native adapter; authentication ID is never projected.
            Identity=BinaryPrimitives.ReadUInt64LittleEndian(bytes.AsSpan(0,8));
            Modified=BinaryPrimitives.ReadUInt64LittleEndian(bytes.AsSpan(48,8));
            uint type=BinaryPrimitives.ReadUInt32LittleEndian(bytes.AsSpan(24,4));
            uint level=BinaryPrimitives.ReadUInt32LittleEndian(bytes.AsSpan(28,4));
            if(type is not (1u or 2u)||(type==2&&level>3))throw new CovenTokenReadFailure(CovenTokenFailure.Layout);
            Type=(int)type;Level=type==2?(int)level:null;
        }
        internal bool Same(Statistics other) => Identity==other.Identity&&Modified==other.Modified&&Type==other.Type&&Level==other.Level;
    }
    private Statistics Stats(IntPtr token) => Read(token,10,56,bytes=>new Statistics(bytes));
    private CovenTokenQueryResult Collect() {
        CovenTokenFailure failure=CovenTokenFailure.None;uint error=0;
        CovenEffectiveTokenFacts? facts=null;
        try {
            IntPtr token=Open(out CovenTokenContext context);
            if(!api.LookupSelectedPrivilege(out uint low,out int high,out uint nativeError))throw new CovenTokenReadFailure(CovenTokenFailure.Lookup,nativeError);
            Statistics before=Stats(token);
            if(before.Type!=(context==CovenTokenContext.Thread?2:1))throw new CovenTokenReadFailure(CovenTokenFailure.Layout);
            int elevationType=Read(token,18,4,bytes=> {
                uint value=BinaryPrimitives.ReadUInt32LittleEndian(bytes);
                if(value<1||value>3)throw new CovenTokenReadFailure(CovenTokenFailure.Layout);
                return (int)value;
            });
            bool elevated=Read(token,20,4,bytes=>BinaryPrimitives.ReadUInt32LittleEndian(bytes)!=0);
            bool probe=api.Query(token,3,IntPtr.Zero,0,out int bytes,out uint probeError);
            if(probe||probeError!=122)throw new CovenTokenReadFailure(CovenTokenFailure.Probe,probe?0:probeError);
            var privilege=Read(token,3,bytes,buffer=>CovenTokenPrivilegeProjector.Project(buffer,buffer.Length,low,high));
            Statistics after=Stats(token);
            if(!before.Same(after))throw new CovenTokenReadFailure(CovenTokenFailure.ChangedContext);
            IntPtr endpoint=Open(out CovenTokenContext endpointContext);
            Statistics reopened=Stats(endpoint);
            if(reopened.Type!=(endpointContext==CovenTokenContext.Thread?2:1))throw new CovenTokenReadFailure(CovenTokenFailure.Layout);
            if(context!=endpointContext||!before.Same(reopened))throw new CovenTokenReadFailure(CovenTokenFailure.ChangedContext);
            facts=new CovenEffectiveTokenFacts(context,before.Type,before.Level,elevationType,elevated,privilege);
        } catch(CovenTokenReadFailure e){failure=e.Failure;error=e.NativeError;}
          catch{failure=CovenTokenFailure.Unexpected;}
        finally {
            for(int i=handles.Count-1;i>=0;i--) {
                closeAttempts++;
                try{if(!api.Close(handles[i]))closeFailures++;}catch{closeFailures++;}
            }
        }
        return new CovenTokenQueryResult(failure,error,clearAttempts,clearFailures,freeAttempts,freeFailures,closeAttempts,closeFailures,facts);
    }
}
