#nullable enable
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Runtime.InteropServices;

public interface ICovenKernelApi {
    IntPtr OpenPublisher(out uint error);
    IntPtr OpenEnumeration(IntPtr publisher,out uint error);
    IntPtr Next(IntPtr enumeration,out uint error);
    bool Property(IntPtr handle,bool publisher,uint property,uint size,IntPtr buffer,out uint used,out uint error);
    bool Close(IntPtr handle,out uint error);
}
public interface ICovenKernelBuffers { IntPtr Allocate(int size); void Free(IntPtr buffer); }
public sealed class CovenKernelBuffers : ICovenKernelBuffers {
    public IntPtr Allocate(int size) { return Marshal.AllocHGlobal(size); }
    public void Free(IntPtr buffer) { Marshal.FreeHGlobal(buffer); }
}
public sealed class CovenKernelOperation {
    public string ErrorClass { get; set; } = "none";
    public uint? NativeError { get; set; }
    public string FailedStage { get; set; } = "none";
    public uint? FailedProperty { get; set; }
}
public sealed class CovenKernelCleanup {
    public int AttemptedHandles { get; set; }
    public int FailedHandles { get; set; }
    public uint? FirstNativeError { get; set; }
    public int AttemptedBuffers { get; set; }
    public int FailedBuffers { get; set; }
    public bool Complete { get { return FailedHandles==0 && FailedBuffers==0; } }
}
public sealed class CovenKernelEvent {
    public uint Id { get; set; }
    public uint Version { get; set; }
    public uint Channel { get; set; }
    public uint Level { get; set; }
    public uint Opcode { get; set; }
    public uint Task { get; set; }
    public string KeywordsRawHex { get; set; } = "0000000000000000";
    public string KeywordsProviderHex { get; set; } = "0000000000000000";
    public CovenKernelTemplateResult Template { get; set; } = new CovenKernelTemplateResult();
}
public sealed class CovenKernelCollector {
    public string ProviderCode { get { return "kernel-file"; } }
    public bool ProviderMatched { get; set; }
    public bool EnumerationComplete { get; set; }
    public string DescriptorScope { get { return "numeric-event-metadata"; } }
    public string TemplateScope { get { return "template-data-only-no-userdata"; } }
    public int ObservedEventHandles { get; set; }
    public int RetainedEventCount { get { return Events.Count; } }
    public int PrivateBufferBytes { get; set; }
    public int TotalProjectedFields { get; set; }
    public bool Truncated { get; set; }
    public CovenKernelOperation Operation { get; } = new CovenKernelOperation();
    public CovenKernelCleanup Cleanup { get; } = new CovenKernelCleanup();
    public List<CovenKernelEvent> Events { get; } = new List<CovenKernelEvent>();
}
public static class CovenKernelReader {
    private sealed class Failed : Exception { }
    private static void Fail(CovenKernelCollector result,string code,string stage,uint? property=null,uint? error=null) {
        if (result.Operation.ErrorClass=="none") {
            result.Operation.ErrorClass=code;result.Operation.FailedStage=stage;
            result.Operation.FailedProperty=property;result.Operation.NativeError=error;
        }
        throw new Failed();
    }
    private static long PayloadOffset(IntPtr buffer,uint used,IntPtr pointer,int alignment,int length) {
        long start=buffer.ToInt64(),address=pointer.ToInt64();
        if (pointer==IntPtr.Zero || address<start || address%alignment!=0) throw new Failed();
        long offset=checked(address-start);
        if (offset<16 || offset>used || length>used-offset) throw new Failed();
        return offset;
    }
    private static object? Read(CovenKernelCollector result,ICovenKernelApi api,ICovenKernelBuffers buffers,IntPtr handle,bool publisher,uint property) {
        string stage=publisher?"provider-guid":"event-property";
        uint size,error;
        bool probed=api.Property(handle,publisher,property,0,IntPtr.Zero,out size,out error);
        if (probed) Fail(result,"property-probe-failed",stage,property);
        if (error!=122) Fail(result,"property-probe-failed",stage,property,error);
        if (size<16 || size>65536) Fail(result,"property-size-limit",stage,property);
        if (size>1048576-result.PrivateBufferBytes) Fail(result,"total-size-limit",stage,property);
        result.PrivateBufferBytes+=checked((int)size);
        IntPtr buffer=buffers.Allocate(checked((int)size));
        if (buffer==IntPtr.Zero) Fail(result,"collector-failed",stage,property);
        try {
            uint used;
            if (!api.Property(handle,publisher,property,size,buffer,out used,out error))
                Fail(result,error==122?"property-size-changed":"property-read-failed",stage,property,error);
            if (used>size) Fail(result,"property-size-changed",stage,property);
            if (used<16) Fail(result,"variant-invalid",stage,property);
            uint type=unchecked((uint)Marshal.ReadInt32(buffer,12));
            uint expected=publisher?15u:property==8?1u:property==6?10u:8u;
            if (type!=expected) Fail(result,"variant-invalid",stage,property);
            // Scalar Count is intentionally ignored. Array/extra flags failed exact Type.
            if (publisher) {
                IntPtr pointer=Marshal.ReadIntPtr(buffer);
                try { PayloadOffset(buffer,used,pointer,4,16); }
                catch (Exception e) when(e is Failed || e is OverflowException) { Fail(result,"variant-invalid",stage,property); }
                byte[] bytes=new byte[16];Marshal.Copy(pointer,bytes,0,16);return new Guid(bytes);
            }
            if (property==8) {
                IntPtr pointer=Marshal.ReadIntPtr(buffer);
                if (pointer==IntPtr.Zero) return null;
                long offset=0;
                try { offset=PayloadOffset(buffer,used,pointer,2,2); }
                catch (Exception e) when(e is Failed || e is OverflowException) { Fail(result,"variant-invalid",stage,property); }
                int maximum=checked((int)((used-offset)/2));
                int count=0;
                while (count<maximum && Marshal.ReadInt16(pointer,count*2)!=0) count++;
                if (count==maximum) Fail(result,"variant-invalid",stage,property);
                if (count>32768) Fail(result,"template-limit","template");
                return Marshal.PtrToStringUni(pointer,count);
            }
            if (property==6) return unchecked((ulong)Marshal.ReadInt64(buffer));
            return unchecked((uint)Marshal.ReadInt32(buffer));
        } finally {
            result.Cleanup.AttemptedBuffers++;
            try { buffers.Free(buffer); }
            catch { result.Cleanup.FailedBuffers++; }
        }
    }
    private static uint Descriptor(CovenKernelCollector result,ICovenKernelApi api,ICovenKernelBuffers buffers,IntPtr item,uint property) {
        uint value=(uint)Read(result,api,buffers,item,false,property)!;
        uint max=property==0 || property==5?65535u:255u;
        if (value>max) Fail(result,"descriptor-range","event-property",property);
        return value;
    }
    private static void Close(CovenKernelCollector result,ICovenKernelApi api,IntPtr handle) {
        if (handle==IntPtr.Zero) return;
        result.Cleanup.AttemptedHandles++;
        uint error=0;
        bool success=false;
        try { success=api.Close(handle,out error); }
        catch { /* Only fixed cleanup counters leave this boundary. */ }
        if (!success) {
            result.Cleanup.FailedHandles++;
            if (result.Cleanup.FailedHandles==1 && error!=0) result.Cleanup.FirstNativeError=error;
        }
    }
    public static CovenKernelCollector Collect(ICovenKernelApi api,ICovenKernelBuffers buffers) {
        var result=new CovenKernelCollector();
        IntPtr publisher=IntPtr.Zero,enumeration=IntPtr.Zero;
        try {
            uint error;
            publisher=api.OpenPublisher(out error);
            if (publisher==IntPtr.Zero) Fail(result,"provider-open-failed","provider-open",null,error);
            Guid guid=(Guid)Read(result,api,buffers,publisher,true,0)!;
            if (guid!=new Guid("edd08927-9cc4-4e65-b970-c2560fb5c289")) Fail(result,"provider-mismatch","provider-guid",0);
            result.ProviderMatched=true;
            enumeration=api.OpenEnumeration(publisher,out error);
            if (enumeration==IntPtr.Zero) Fail(result,"enumeration-open-failed","enumeration-open",null,error);
            while (true) {
                IntPtr item=api.Next(enumeration,out error);
                if (item==IntPtr.Zero) {
                    if (error==259) { result.EnumerationComplete=true;break; }
                    Fail(result,"enumeration-failed","enumeration-next",null,error);
                }
                result.ObservedEventHandles++;
                try {
                    if (result.Events.Count==256) { result.Truncated=true;Fail(result,"event-limit","enumeration-next"); }
                    var definition=new CovenKernelEvent {
                        Id=Descriptor(result,api,buffers,item,0),Version=Descriptor(result,api,buffers,item,1),
                        Channel=Descriptor(result,api,buffers,item,2),Level=Descriptor(result,api,buffers,item,3),
                        Opcode=Descriptor(result,api,buffers,item,4),Task=Descriptor(result,api,buffers,item,5)
                    };
                    ulong keywords=(ulong)Read(result,api,buffers,item,false,6)!;
                    definition.KeywordsRawHex=keywords.ToString("x16",CultureInfo.InvariantCulture);
                    definition.KeywordsProviderHex=(keywords&0x0000ffffffffffffUL).ToString("x16",CultureInfo.InvariantCulture);
                    string? xml=(string?)Read(result,api,buffers,item,false,8);
                    definition.Template=CovenKernelTemplate.Project(xml,2048-result.TotalProjectedFields);
                    if (definition.Template.Status=="limit") Fail(result,"template-limit","template");
                    result.TotalProjectedFields+=definition.Template.Fields.Count;
                    result.Events.Add(definition);
                } finally { Close(result,api,item); }
            }
        } catch (Failed) { }
        catch {
            if (result.Operation.ErrorClass=="none") {
                result.Operation.ErrorClass="collector-failed";result.Operation.FailedStage="none";
            }
            // Unexpected managed failures withhold all definitions and private errors.
            result.Events.Clear();result.TotalProjectedFields=0;
        } finally {
            Close(result,api,enumeration);
            Close(result,api,publisher);
        }
        return result;
    }
}

// Fixed read-only local API surface. Controlled tests inspect imports, never call them.
public sealed class CovenWindowsKernelApi : ICovenKernelApi {
    public CovenWindowsKernelApi() {
        if (!OperatingSystem.IsWindows() || IntPtr.Size!=8) throw new PlatformNotSupportedException();
    }
    [DllImport("wevtapi.dll",ExactSpelling=true,CharSet=CharSet.Unicode,SetLastError=true)]
    [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    private static extern IntPtr EvtOpenPublisherMetadata(IntPtr session,string publisher,string? logFile,uint locale,uint flags);
    [DllImport("wevtapi.dll",ExactSpelling=true,SetLastError=true)]
    [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    private static extern IntPtr EvtOpenEventMetadataEnum(IntPtr publisher,uint flags);
    [DllImport("wevtapi.dll",ExactSpelling=true,SetLastError=true)]
    [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    private static extern IntPtr EvtNextEventMetadata(IntPtr enumeration,uint flags);
    [DllImport("wevtapi.dll",ExactSpelling=true,SetLastError=true)]
    [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    [return:MarshalAs(UnmanagedType.Bool)]
    private static extern bool EvtGetPublisherMetadataProperty(IntPtr publisher,uint property,uint flags,uint size,IntPtr buffer,out uint used);
    [DllImport("wevtapi.dll",ExactSpelling=true,SetLastError=true)]
    [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    [return:MarshalAs(UnmanagedType.Bool)]
    private static extern bool EvtGetEventMetadataProperty(IntPtr item,uint property,uint flags,uint size,IntPtr buffer,out uint used);
    [DllImport("wevtapi.dll",ExactSpelling=true,SetLastError=true)]
    [DefaultDllImportSearchPaths(DllImportSearchPath.System32)]
    [return:MarshalAs(UnmanagedType.Bool)]
    private static extern bool EvtClose(IntPtr handle);
    public IntPtr OpenPublisher(out uint error) {
        IntPtr handle=EvtOpenPublisherMetadata(IntPtr.Zero,"Microsoft-Windows-Kernel-File",null,0,0);
        error=handle==IntPtr.Zero?unchecked((uint)Marshal.GetLastPInvokeError()):0;
        return handle;
    }
    public IntPtr OpenEnumeration(IntPtr publisher,out uint error) {
        IntPtr handle=EvtOpenEventMetadataEnum(publisher,0);
        error=handle==IntPtr.Zero?unchecked((uint)Marshal.GetLastPInvokeError()):0;
        return handle;
    }
    public IntPtr Next(IntPtr enumeration,out uint error) {
        IntPtr handle=EvtNextEventMetadata(enumeration,0);
        error=handle==IntPtr.Zero?unchecked((uint)Marshal.GetLastPInvokeError()):0;
        return handle;
    }
    public bool Property(IntPtr handle,bool publisher,uint property,uint size,IntPtr buffer,out uint used,out uint error) {
        bool success=publisher?EvtGetPublisherMetadataProperty(handle,property,0,size,buffer,out used):EvtGetEventMetadataProperty(handle,property,0,size,buffer,out used);
        error=success?0:unchecked((uint)Marshal.GetLastPInvokeError());
        return success;
    }
    public bool Close(IntPtr handle,out uint error) {
        bool success=EvtClose(handle);
        error=success?0:unchecked((uint)Marshal.GetLastPInvokeError());
        return success;
    }
}
