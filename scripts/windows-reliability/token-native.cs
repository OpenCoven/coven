#nullable enable
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

// The injected implementation exists for portable controls only. Production
// construction selects the fixed sealed platform implementation below.
internal interface ICovenTokenNativeCalls {
    IntPtr CurrentThread();
    IntPtr CurrentProcess();
    bool OpenThread(IntPtr thread,uint access,bool asSelf,out IntPtr opened);
    bool OpenProcess(IntPtr process,uint access,out IntPtr opened);
    bool Lookup(string name,out uint low,out int high);
    bool Query(IntPtr opened,int info,IntPtr buffer,int bytes,out uint returned);
    bool Close(IntPtr opened);
    uint LastError();
}
internal sealed class CovenTokenNativeApi : ICovenTokenQueryApi {
    private readonly ICovenTokenNativeCalls calls;
    private readonly HashSet<IntPtr> handles=new(2);
    private readonly Dictionary<IntPtr,int> allocations=new();
    private readonly byte[] zeroes=new byte[4096];
    private int requested;
    internal CovenTokenNativeApi() {
        if(!OperatingSystem.IsWindows())throw new PlatformNotSupportedException();
        ValidateLayout();calls=new PlatformCalls();
    }
    internal CovenTokenNativeApi(ICovenTokenNativeCalls calls) { ValidateLayout();this.calls=calls; }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativeLuid { internal uint Low;internal int High; }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativePrivilege { internal NativeLuid Luid;internal uint Attributes; }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativePrivileges { internal uint Count;internal NativePrivilege First; }
    [StructLayout(LayoutKind.Sequential)]
    private struct NativeStatistics {
        internal NativeLuid Identity,Authentication;
        internal long Expiration;
        internal int Type,Level;
        internal uint DynamicCharged,DynamicAvailable,GroupCount,PrivilegeCount;
        internal NativeLuid Modified;
    }
    internal static void ValidateLayout() {
        static void Size<T>(int bytes){if(Marshal.SizeOf<T>()!=bytes)throw new PlatformNotSupportedException("Token layout unsupported");}
        static void Offset<T>(string field,int bytes){if(Marshal.OffsetOf<T>(field).ToInt64()!=bytes)throw new PlatformNotSupportedException("Token layout unsupported");}
        Size<NativeLuid>(8);Offset<NativeLuid>(nameof(NativeLuid.Low),0);Offset<NativeLuid>(nameof(NativeLuid.High),4);
        Size<NativePrivilege>(12);Offset<NativePrivilege>(nameof(NativePrivilege.Luid),0);Offset<NativePrivilege>(nameof(NativePrivilege.Attributes),8);
        Size<NativePrivileges>(16);Offset<NativePrivileges>(nameof(NativePrivileges.Count),0);Offset<NativePrivileges>(nameof(NativePrivileges.First),4);
        Size<NativeStatistics>(56);Offset<NativeStatistics>(nameof(NativeStatistics.Identity),0);Offset<NativeStatistics>(nameof(NativeStatistics.Type),24);Offset<NativeStatistics>(nameof(NativeStatistics.Level),28);Offset<NativeStatistics>(nameof(NativeStatistics.Modified),48);
        if(!BitConverter.IsLittleEndian||Marshal.SizeOf<uint>()!=4)throw new PlatformNotSupportedException("Token layout unsupported");
    }
    private static bool Invalid(IntPtr value) => value==IntPtr.Zero||value==new IntPtr(-1)||value==new IntPtr(-2);
    private void BeforeOpen(){if(handles.Count>=2)throw new InvalidOperationException("Token open bound exceeded");}
    private void Own(IntPtr value){if(!Invalid(value))handles.Add(value);}
    bool ICovenTokenQueryApi.OpenCurrentThreadToken(out IntPtr opened,out uint error) {
        BeforeOpen();bool ok=calls.OpenThread(calls.CurrentThread(),8,true,out opened);
        error=ok?0:calls.LastError();
        if(ok)Own(opened);else opened=IntPtr.Zero;
        return ok;
    }
    bool ICovenTokenQueryApi.OpenCurrentProcessToken(out IntPtr opened,out uint error) {
        BeforeOpen();bool ok=calls.OpenProcess(calls.CurrentProcess(),8,out opened);
        error=ok?0:calls.LastError();
        if(ok)Own(opened);else opened=IntPtr.Zero;
        return ok;
    }
    bool ICovenTokenQueryApi.LookupSelectedPrivilege(out uint low,out int high,out uint error) {
        bool ok=calls.Lookup("SeSystemProfilePrivilege",out low,out high);
        error=ok?0:calls.LastError();
        if(!ok){low=0;high=0;}return ok;
    }
    IntPtr ICovenTokenQueryApi.Allocate(int bytes) {
        if(bytes<4||bytes>4096||requested>16384-bytes)throw new ArgumentException("Token allocation bound exceeded");
        requested=checked(requested+bytes);
        IntPtr buffer=Marshal.AllocHGlobal(bytes);
        if(Invalid(buffer))throw new InvalidOperationException("Token allocation failed");
        try{allocations.Add(buffer,bytes);}
        catch{try{Marshal.Copy(zeroes,0,buffer,bytes);}finally{Marshal.FreeHGlobal(buffer);}throw;}
        return buffer;
    }
    private void Buffer(IntPtr buffer,int bytes) {
        if(!allocations.TryGetValue(buffer,out int ownedBytes)||ownedBytes!=bytes)throw new ArgumentException("Token buffer is not owned");
    }
    bool ICovenTokenQueryApi.Query(IntPtr opened,int info,IntPtr buffer,int bytes,out int returned,out uint error) {
        if(!handles.Contains(opened)||info is not (3 or 10 or 18 or 20))throw new ArgumentException("Token query is not admitted");
        if(buffer==IntPtr.Zero){if(info!=3||bytes!=0)throw new ArgumentException("Token probe is not admitted");}
        else{Buffer(buffer,bytes);if((info==10&&bytes!=56)||((info==18||info==20)&&bytes!=4))throw new ArgumentException("Token fixed buffer invalid");}
        bool ok=calls.Query(opened,info,buffer,bytes,out uint rawReturned);
        error=ok?0:calls.LastError();
        returned=rawReturned>int.MaxValue?int.MaxValue:(int)rawReturned;
        return ok;
    }
    byte[] ICovenTokenQueryApi.Copy(IntPtr buffer,int bytes) {
        Buffer(buffer,bytes);var copy=new byte[bytes];
        try{Marshal.Copy(buffer,copy,0,bytes);return copy;}catch{Array.Clear(copy);throw;}
    }
    bool ICovenTokenQueryApi.Clear(IntPtr buffer,int bytes) {
        Buffer(buffer,bytes);Marshal.Copy(zeroes,0,buffer,bytes);return true;
    }
    bool ICovenTokenQueryApi.Free(IntPtr buffer) {
        if(!allocations.ContainsKey(buffer))throw new ArgumentException("Token buffer is not owned");
        Marshal.FreeHGlobal(buffer);allocations.Remove(buffer);return true;
    }
    bool ICovenTokenQueryApi.Close(IntPtr opened) {
        // A failed close is uncertain, so no implicit retry/double-close.
        if(!handles.Remove(opened))throw new ArgumentException("Token handle is not owned");
        bool ok=calls.Close(opened);
        if(!ok)_=calls.LastError(); // Capture immediately; public cleanup retains counts only.
        return ok;
    }
    private sealed class PlatformCalls : ICovenTokenNativeCalls {
        public IntPtr CurrentThread()=>Imports.GetCurrentThread();
        public IntPtr CurrentProcess()=>Imports.GetCurrentProcess();
        public bool OpenThread(IntPtr thread,uint access,bool asSelf,out IntPtr opened)=>Imports.OpenThreadToken(thread,access,asSelf,out opened);
        public bool OpenProcess(IntPtr process,uint access,out IntPtr opened)=>Imports.OpenProcessToken(process,access,out opened);
        public bool Lookup(string name,out uint low,out int high) {
            bool ok=Imports.LookupPrivilegeValueW(null,name,out NativeLuid value);low=value.Low;high=value.High;return ok;
        }
        public bool Query(IntPtr opened,int info,IntPtr buffer,int bytes,out uint returned)=>Imports.GetTokenInformation(opened,info,buffer,(uint)bytes,out returned);
        public bool Close(IntPtr opened)=>Imports.CloseHandle(opened);
        public uint LastError()=>unchecked((uint)Marshal.GetLastPInvokeError());
    }
    private static class Imports {
        [DllImport("kernel32.dll",ExactSpelling=true)]
        [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
        internal static extern IntPtr GetCurrentThread();
        [DllImport("kernel32.dll",ExactSpelling=true)]
        [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
        internal static extern IntPtr GetCurrentProcess();
        [DllImport("advapi32.dll",ExactSpelling=true,SetLastError=true)]
        [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
        [return:MarshalAs(UnmanagedType.Bool)]
        internal static extern bool OpenThreadToken(IntPtr thread,uint access,[MarshalAs(UnmanagedType.Bool)] bool asSelf,out IntPtr opened);
        [DllImport("advapi32.dll",ExactSpelling=true,SetLastError=true)]
        [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
        [return:MarshalAs(UnmanagedType.Bool)]
        internal static extern bool OpenProcessToken(IntPtr process,uint access,out IntPtr opened);
        [DllImport("advapi32.dll",ExactSpelling=true,SetLastError=true,CharSet=CharSet.Unicode)]
        [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
        [return:MarshalAs(UnmanagedType.Bool)]
        internal static extern bool LookupPrivilegeValueW(string? system,string name,out NativeLuid luid);
        [DllImport("advapi32.dll",ExactSpelling=true,SetLastError=true)]
        [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
        [return:MarshalAs(UnmanagedType.Bool)]
        internal static extern bool GetTokenInformation(IntPtr opened,int info,IntPtr buffer,uint bytes,out uint returned);
        [DllImport("kernel32.dll",ExactSpelling=true,SetLastError=true)]
        [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
        [return:MarshalAs(UnmanagedType.Bool)]
        internal static extern bool CloseHandle(IntPtr opened);
    }
}
