#nullable enable
using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Runtime.InteropServices;

internal sealed class ControlledTokenCalls : ICovenTokenNativeCalls {
    public readonly List<string> Calls=new();
    public bool FailThread,FailProcess,FailLookup,FailQuery,FailClose,FillFacts,DifferentHandles;
    private uint pendingError;
    private int opens;
    public IntPtr ReturnedHandle=new(100);
    public uint ReturnedBytes=4;
    public uint FailureError=5;
    public IntPtr CurrentThread(){Calls.Add("thread");return new IntPtr(-2);}
    public IntPtr CurrentProcess(){Calls.Add("process");return new IntPtr(-1);}
    public bool OpenThread(IntPtr thread,uint access,bool asSelf,out IntPtr opened){
        Calls.Add("open-thread");if(thread!=new IntPtr(-2)||access!=8||!asSelf)throw new InvalidOperationException();opened=DifferentHandles?new IntPtr(100+opens++):ReturnedHandle;pendingError=FailureError;return !FailThread;
    }
    public bool OpenProcess(IntPtr process,uint access,out IntPtr opened){
        Calls.Add("open-process");if(process!=new IntPtr(-1)||access!=8)throw new InvalidOperationException();opened=DifferentHandles?new IntPtr(100+opens++):ReturnedHandle;pendingError=FailureError;return !FailProcess;
    }
    public bool Lookup(string name,out uint low,out int high){Calls.Add("lookup");if(name!="SeSystemProfilePrivilege")throw new InvalidOperationException();low=7;high=-9;pendingError=FailureError;return !FailLookup;}
    public bool Query(IntPtr opened,int info,IntPtr buffer,int bytes,out uint returned){Calls.Add("query");returned=ReturnedBytes;pendingError=FailureError;
        if(FillFacts){
            if(buffer==IntPtr.Zero){returned=16;pendingError=122;return false;}
            returned=(uint)bytes;
            if(info==10){Marshal.WriteInt64(buffer,0,1);Marshal.WriteInt32(buffer,24,FailThread?1:2);Marshal.WriteInt32(buffer,28,2);Marshal.WriteInt64(buffer,48,1);}
            else if(info==18)Marshal.WriteInt32(buffer,3);
            else if(info==20)Marshal.WriteInt32(buffer,2);
            else if(info==3){Marshal.WriteInt32(buffer,1);Marshal.WriteInt32(buffer,4,7);Marshal.WriteInt32(buffer,8,-9);Marshal.WriteInt32(buffer,12,1);}
        }
        return !FailQuery;}
    public bool Close(IntPtr opened){Calls.Add("close");pendingError=FailureError;return !FailClose;}
    public uint LastError(){Calls.Add("error");return pendingError;}
}
public sealed class CovenTokenNativeControlReport {
    public int Cases { get; }
    public string[] Failures { get; }
    public CovenTokenNativeControlReport(int cases,string[] failures){Cases=cases;Failures=failures;}
}
public static class CovenTokenNativeControls {
    private static void Require(bool value){if(!value)throw new InvalidOperationException("Controlled assertion failed");}
    private static void Reject(Action action){try{action();}catch(ArgumentException){return;}catch(InvalidOperationException){return;}throw new Exception("Unowned call admitted");}
    public static CovenTokenNativeControlReport Run(){
        int cases=0;var failures=new List<string>();
        void Case(string name,Action action){cases++;try{action();}catch{failures.Add(name);}}
        Case("declared-layout",()=>CovenTokenNativeApi.ValidateLayout());
        Case("closed-import-surface",()=>{
            var imports=typeof(CovenTokenNativeApi).Assembly.GetTypes().Where(t=>t.Namespace==typeof(CovenTokenNativeApi).Namespace).SelectMany(t=>t.GetMethods(BindingFlags.Public|BindingFlags.NonPublic|BindingFlags.Static)).Select(m=>(Method:m,Import:m.GetCustomAttribute<DllImportAttribute>())).Where(x=>x.Import is not null).ToArray();
            Require(imports.Length==7);
            Require(imports.Select(x=>x.Import!.EntryPoint).OrderBy(x=>x,StringComparer.Ordinal).SequenceEqual(new[]{"CloseHandle","GetCurrentProcess","GetCurrentThread","GetTokenInformation","LookupPrivilegeValueW","OpenProcessToken","OpenThreadToken"}));
            foreach(var item in imports){
                var a=item.Import!;Require(a.ExactSpelling&&item.Method.GetCustomAttribute<DefaultDllImportSearchPathsAttribute>()?.Paths==DllImportSearchPath.System32);
                Require(a.Value==(a.EntryPoint is "CloseHandle" or "GetCurrentProcess" or "GetCurrentThread"?"kernel32.dll":"advapi32.dll"));
                Require(a.SetLastError==(a.EntryPoint is not ("GetCurrentThread" or "GetCurrentProcess")));
                if(a.EntryPoint=="LookupPrivilegeValueW")Require(a.CharSet==CharSet.Unicode);
                bool pseudo=a.EntryPoint is "GetCurrentThread" or "GetCurrentProcess";
                Require(item.Method.ReturnType==(pseudo?typeof(IntPtr):typeof(bool)));
                if(!pseudo)Require(item.Method.ReturnParameter.GetCustomAttribute<MarshalAsAttribute>()?.Value==UnmanagedType.Bool);
                if(a.EntryPoint=="OpenThreadToken"){
                    var parameters=item.Method.GetParameters();
                    Require(parameters.Select(p=>p.ParameterType).SequenceEqual(new[]{typeof(IntPtr),typeof(uint),typeof(bool),typeof(IntPtr).MakeByRefType()}));
                    Require(parameters[2].GetCustomAttribute<MarshalAsAttribute>()?.Value==UnmanagedType.Bool);
                }
                if(a.EntryPoint=="GetTokenInformation")Require(item.Method.GetParameters().Select(p=>p.ParameterType).SequenceEqual(new[]{typeof(IntPtr),typeof(int),typeof(IntPtr),typeof(uint),typeof(uint).MakeByRefType()}));
            }
        });
        Case("thread-query-only",()=>{var c=new ControlledTokenCalls();ICovenTokenQueryApi a=new CovenTokenNativeApi(c);Require(a.OpenCurrentThreadToken(out var h,out var error)&&h==new IntPtr(100)&&error==0&&c.Calls.SequenceEqual(new[]{"thread","open-thread"}));Require(a.Close(h));});
        Case("process-query-only",()=>{var c=new ControlledTokenCalls();ICovenTokenQueryApi a=new CovenTokenNativeApi(c);Require(a.OpenCurrentProcessToken(out var h,out var error)&&error==0&&c.Calls.SequenceEqual(new[]{"process","open-process"}));a.Close(h);});
        Case("failed-thread-error-immediate-no-owned-output",()=>{var c=new ControlledTokenCalls{FailThread=true};ICovenTokenQueryApi a=new CovenTokenNativeApi(c);Require(!a.OpenCurrentThreadToken(out var h,out var error)&&h==IntPtr.Zero&&error==5&&c.Calls.SequenceEqual(new[]{"thread","open-thread","error"}));Reject(()=>a.Close(new IntPtr(100)));});
        Case("failed-process-error-immediate",()=>{var c=new ControlledTokenCalls{FailProcess=true};ICovenTokenQueryApi a=new CovenTokenNativeApi(c);Require(!a.OpenCurrentProcessToken(out var h,out var error)&&h==IntPtr.Zero&&error==5&&c.Calls.SequenceEqual(new[]{"process","open-process","error"}));});
        Case("fixed-local-lookup",()=>{var c=new ControlledTokenCalls();ICovenTokenQueryApi a=new CovenTokenNativeApi(c);Require(a.LookupSelectedPrivilege(out var low,out var high,out var error)&&low==7&&high==-9&&error==0&&c.Calls.SequenceEqual(new[]{"lookup"}));});
        Case("lookup-error-immediate",()=>{var c=new ControlledTokenCalls{FailLookup=true};ICovenTokenQueryApi a=new CovenTokenNativeApi(c);Require(!a.LookupSelectedPrivilege(out var low,out var high,out var error)&&low==0&&high==0&&error==5&&c.Calls.SequenceEqual(new[]{"lookup","error"}));});
        Case("owned-memory-clear-copy-free",()=>{ICovenTokenQueryApi a=new CovenTokenNativeApi(new ControlledTokenCalls());var p=a.Allocate(16);Marshal.WriteInt32(p,12345);var before=a.Copy(p,16);Require(BitConverter.ToInt32(before,0)==12345);Require(a.Clear(p,16));Require(a.Copy(p,16).All(b=>b==0));Require(a.Free(p));Reject(()=>a.Free(p));Reject(()=>a.Copy(p,16));Array.Clear(before);});
        foreach(int bytes in new[]{0,3,4097,int.MaxValue})Case("allocation-bound-"+bytes,()=>{ICovenTokenQueryApi a=new CovenTokenNativeApi(new ControlledTokenCalls());Reject(()=>a.Allocate(bytes));});
        Case("cumulative-cap-after-frees",()=>{ICovenTokenQueryApi a=new CovenTokenNativeApi(new ControlledTokenCalls());for(int i=0;i<4;i++){var p=a.Allocate(4096);a.Clear(p,4096);a.Free(p);}Reject(()=>a.Allocate(4));});
        Case("foreign-memory-rejected-before-native",()=>{var c=new ControlledTokenCalls();ICovenTokenQueryApi a=new CovenTokenNativeApi(c);Reject(()=>a.Clear(new IntPtr(88),4));Reject(()=>a.Free(new IntPtr(88)));Reject(()=>a.Copy(new IntPtr(88),4));Require(c.Calls.Count==0);});
        Case("owned-buffer-size-must-match",()=>{ICovenTokenQueryApi a=new CovenTokenNativeApi(new ControlledTokenCalls());var p=a.Allocate(16);try{Reject(()=>a.Copy(p,4));Reject(()=>a.Clear(p,4));}finally{a.Clear(p,16);a.Free(p);}});
        Case("probe-and-failed-read-errors",()=>{var c=new ControlledTokenCalls{FailQuery=true,FailureError=122};ICovenTokenQueryApi a=new CovenTokenNativeApi(c);a.OpenCurrentThreadToken(out var h,out _);try{Require(!a.Query(h,3,IntPtr.Zero,0,out var bytes,out var error)&&bytes==4&&error==122&&c.Calls.TakeLast(2).SequenceEqual(new[]{"query","error"}));}finally{a.Close(h);}});
        Case("successful-read-no-stale-error",()=>{var c=new ControlledTokenCalls();ICovenTokenQueryApi a=new CovenTokenNativeApi(c);a.OpenCurrentThreadToken(out var h,out _);var p=a.Allocate(4);try{Require(a.Query(h,20,p,4,out var bytes,out var error)&&bytes==4&&error==0&&!c.Calls.Contains("error"));}finally{a.Clear(p,4);a.Free(p);a.Close(h);}});
        Case("buffer-size-query-rejected-before-native",()=>{var c=new ControlledTokenCalls();ICovenTokenQueryApi a=new CovenTokenNativeApi(c);a.OpenCurrentThreadToken(out var h,out _);var p=a.Allocate(16);try{Reject(()=>a.Query(h,20,p,16,out _,out _));Reject(()=>a.Query(h,3,p,4,out _,out _));Require(!c.Calls.Contains("query"));}finally{a.Clear(p,16);a.Free(p);a.Close(h);}});
        Case("platform-constructor-no-native-call",()=>{if(!OperatingSystem.IsWindows()){try{_ = new CovenTokenNativeApi();}catch(PlatformNotSupportedException){return;}throw new Exception();}});
        Case("closed-info-class-and-probe",()=>{var c=new ControlledTokenCalls();ICovenTokenQueryApi a=new CovenTokenNativeApi(c);a.OpenCurrentThreadToken(out var h,out _);try{Reject(()=>a.Query(h,2,IntPtr.Zero,0,out _,out _));Reject(()=>a.Query(h,10,IntPtr.Zero,0,out _,out _));Reject(()=>a.Query(new IntPtr(77),3,IntPtr.Zero,0,out _,out _));Require(!c.Calls.Contains("query"));}finally{a.Close(h);}});
        foreach(long invalid in new[]{0L,-1L,-2L})Case("pseudo-never-owned-"+invalid,()=>{var c=new ControlledTokenCalls{ReturnedHandle=new IntPtr(invalid)};ICovenTokenQueryApi a=new CovenTokenNativeApi(c);a.OpenCurrentThreadToken(out _,out _);Reject(()=>a.Close(new IntPtr(invalid)));Require(!c.Calls.Contains("close"));});
        Case("failed-close-is-not-retried",()=>{var c=new ControlledTokenCalls{FailClose=true};ICovenTokenQueryApi a=new CovenTokenNativeApi(c);a.OpenCurrentThreadToken(out var h,out _);Require(!a.Close(h)&&c.Calls.TakeLast(2).SequenceEqual(new[]{"close","error"}));Reject(()=>a.Close(h));Require(c.Calls.Count(x=>x=="close")==1);});
        Case("reader-through-real-owned-memory",()=>{var c=new ControlledTokenCalls{FillFacts=true,DifferentHandles=true};var r=CovenTokenQueryReader.Collect(new CovenTokenNativeApi(c));Require(r.Failure==CovenTokenFailure.None&&!r.CleanupFailed&&r.Facts?.Context==CovenTokenContext.Thread&&r.Facts.Elevated&&r.Facts.Privilege.EnabledByDefault&&r.ClearAttempts==6&&r.FreeAttempts==6&&r.CloseAttempts==2);});
        Case("reader-process-fallback-through-adapter",()=>{var c=new ControlledTokenCalls{FillFacts=true,DifferentHandles=true,FailThread=true,FailureError=1008};var r=CovenTokenQueryReader.Collect(new CovenTokenNativeApi(c));Require(r.Failure==CovenTokenFailure.None&&r.Facts?.Context==CovenTokenContext.Process&&r.Facts.TokenType==1&&r.Facts.ImpersonationLevel is null);});
        Case("huge-native-length-does-not-wrap",()=>{var c=new ControlledTokenCalls{FailQuery=true,ReturnedBytes=uint.MaxValue,FailureError=122};ICovenTokenQueryApi a=new CovenTokenNativeApi(c);a.OpenCurrentThreadToken(out var h,out _);try{Require(!a.Query(h,3,IntPtr.Zero,0,out var bytes,out var error)&&bytes==int.MaxValue&&error==122);}finally{a.Close(h);}});
        Case("duplicate-open-owns-once",()=>{var c=new ControlledTokenCalls();ICovenTokenQueryApi a=new CovenTokenNativeApi(c);a.OpenCurrentThreadToken(out var h,out _);a.OpenCurrentThreadToken(out var same,out _);Require(h==same);a.Close(h);Reject(()=>a.Close(same));Require(c.Calls.Count(x=>x=="close")==1);});
        return new CovenTokenNativeControlReport(cases,failures.ToArray());
    }
}
