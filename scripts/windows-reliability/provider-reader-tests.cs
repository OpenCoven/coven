#nullable enable
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public sealed class ControlledKernelApi : ICovenKernelApi {
    public string Fault = "";
    public int EventCount = 1;
    public int Position;
    public int PropertyCalls;
    public int ParentHandles;
    public List<long> Closed = new List<long>();
    public IntPtr OpenPublisher(out uint error) { if (Fault=="throw-open") throw new Exception("PRIVATE_SENTINEL"); error = Fault == "open" ? 5u : 0u; if (error==0) ParentHandles++; return error == 0 ? new IntPtr(1) : IntPtr.Zero; }
    public IntPtr OpenEnumeration(IntPtr publisher, out uint error) { if (Fault=="throw-enum") throw new Exception("PRIVATE_SENTINEL"); error = Fault == "enum-open" ? 5u : 0u; if (error==0) ParentHandles++; return error == 0 ? new IntPtr(2) : IntPtr.Zero; }
    public IntPtr Next(IntPtr enumeration, out uint error) {
        if (Fault=="throw-next") throw new Exception("PRIVATE_SENTINEL");
        if (Fault == "next") { error=5; return IntPtr.Zero; }
        if (Fault == "next-zero") { error=0; return IntPtr.Zero; }
        if (Position >= EventCount) { error=259; return IntPtr.Zero; }
        error=0; return new IntPtr(100 + Position++);
    }
    public bool Close(IntPtr handle, out uint error) {
        long id=handle.ToInt64();
        if (Closed.Contains(id)) throw new Exception("Double close");
        Closed.Add(id);
        if (Fault=="close-zero-then-error" && id>=100) { error=0;return false; }
        if (Fault=="close-error-then-throw" && id==2) throw new Exception("PRIVATE_SENTINEL");
        if ((Fault == "close-throw" || Fault == "close-throw-then-error") && id >= 100) throw new Exception("PRIVATE_SENTINEL");
        error=(Fault=="close-error-then-throw" && id>=100) || Fault=="close-zero-then-error" || Fault == "close-throw-then-error" || Fault == "close-all" || (Fault == "close-event" && id>=100) || (Fault=="close-enum" && id==2) || (Fault=="close-publisher" && id==1) || (Fault=="primary-and-cleanup" && id>=100) ? 5u : 0u;
        return error==0;
    }
    public bool Property(IntPtr handle, bool publisher, uint property, uint size, IntPtr buffer, out uint used, out uint error) {
        if (!publisher && property == 7) throw new Exception("Message metadata must never be read");
        PropertyCalls++;
        if (Fault=="throw-property") throw new Exception("PRIVATE_SENTINEL");
        string xml=Fault=="private" ? "<template><data name=\"PRIVATE_SENTINEL\" inType=\"PRIVATE_SENTINEL\"/></template>" : "<template><data name=\"Irp\" inType=\"win:Pointer\"/></template>";
        if (Fault=="eight-fields") xml="<template>"+string.Concat(System.Linq.Enumerable.Repeat("<data name=\"Irp\" inType=\"win:Pointer\"/>",8))+"</template>";
        if (Fault=="global-fields") xml="<template>"+string.Concat(System.Linq.Enumerable.Repeat("<data name=\"Irp\" inType=\"win:Pointer\"/>",9))+"</template>";
        if (Fault=="template-limit") xml=new string('x',32769);
        used=publisher ? 32u : property==8 ? (uint)(16+(xml.Length+1)*2) : 16u;
        if (Fault=="total-limit" && !publisher) used=65536;
        if (size==0) {
            if (Fault=="smaller-used" && !publisher && property<=6) used=64;
            error=122;
            if (Fault=="probe-success" || Fault=="probe-success-stale") { error=Fault=="probe-success-stale"?5u:0u;return true; }
            if (Fault=="probe-error" || (Fault=="provider-property" && publisher)) { error=5;return false; }
            if (Fault=="small") used=15;
            if (Fault=="large") used=65537;
            return false;
        }
        error=0;
        if (Fault=="read-error") { error=5;return false; }
        if (Fault=="growth") { used=size+2;error=122;return false; }
        if (Fault=="used-over") { used=size+1;return true; }
        if (Fault=="used-short") { used=15;return true; }
        if (Fault=="stale-error") error=5; // Success error is deliberately stale.
        for (int i=0;i<(int)size;i++) Marshal.WriteByte(buffer,i,0);
        Marshal.WriteInt32(buffer,8,77); // Scalar Count must not become a string length.
        uint type=publisher ? 15u : property==8 ? 1u : property==6 ? 10u : 8u;
        if (!publisher && property==0 && (Fault=="type" || Fault=="primary-and-cleanup")) type=1;
        if (Fault=="array" && !publisher) type|=128;
        if (Fault=="extra-type-flags") type|=256;
        Marshal.WriteInt32(buffer,12,unchecked((int)type));
        if (publisher) {
            Guid guid=Fault=="provider-mismatch" ? Guid.Empty : new Guid("edd08927-9cc4-4e65-b970-c2560fb5c289");
            byte[] bytes=guid.ToByteArray();Marshal.Copy(bytes,0,IntPtr.Add(buffer,16),16);
            Marshal.WriteIntPtr(buffer,IntPtr.Add(buffer,16));
            if (Fault=="guid-short") used=31;
            if (Fault=="guid-null") Marshal.WriteIntPtr(buffer,IntPtr.Zero);
            if (Fault=="guid-unaligned") Marshal.WriteIntPtr(buffer,IntPtr.Add(buffer,17));
        } else if (property==8) {
            if (Fault=="null-template") { Marshal.WriteIntPtr(buffer,IntPtr.Zero);return true; }
            byte[] bytes=System.Text.Encoding.Unicode.GetBytes(xml+'\0');Marshal.Copy(bytes,0,IntPtr.Add(buffer,16),bytes.Length);
            IntPtr text=IntPtr.Add(buffer,16);
            if (Fault=="pointer-out") text=IntPtr.Add(buffer,(int)size);
            if (Fault=="pointer-header") text=buffer;
            if (Fault=="pointer-unaligned") text=IntPtr.Add(buffer,17);
            if (Fault=="pointer-negative") text=new IntPtr(-1);
            Marshal.WriteIntPtr(buffer,text);
            if (Fault=="unterminated") Marshal.WriteInt16(buffer,(int)used-2,65);
            if (Fault=="payload-short") used=16;
        } else if (property==6) Marshal.WriteInt64(buffer,unchecked((long)0x8000123456789abcUL));
        else {
            uint value=property==0 ? 42u : 0u;
            if (Fault=="range-id" && property==0) value=65536;
            if (Fault=="range-version" && property==1) value=256;
            if (Fault=="range-channel" && property==2) value=256;
            if (Fault=="range-level" && property==3) value=256;
            if (Fault=="range-opcode" && property==4) value=256;
            if (Fault=="range-task" && property==5) value=65536;
            Marshal.WriteInt32(buffer,unchecked((int)value));
        }
        return true;
    }
}
public sealed class ControlledKernelBuffers : ICovenKernelBuffers {
    public string Fault = "";
    public int FreeAttempts;
    public HashSet<IntPtr> Owned = new HashSet<IntPtr>();
    public IntPtr Allocate(int size) { if (Fault=="allocate-zero") return IntPtr.Zero; if (Fault=="allocate-throw") throw new Exception("PRIVATE_SENTINEL"); var p=Marshal.AllocHGlobal(size);Owned.Add(p);return p; }
    public void Free(IntPtr pointer) { FreeAttempts++; if (Fault=="before") throw new Exception("PRIVATE_SENTINEL"); if (!Owned.Remove(pointer)) throw new Exception("Unowned buffer free"); Marshal.FreeHGlobal(pointer); if (Fault=="after") throw new Exception("PRIVATE_SENTINEL"); }
}
public static class ControlledKernelTests {
    private static int Count;
    private static void Require(bool value,string label) { if (!value) throw new Exception(label); }
    private static CovenKernelCollector Run(string fault,string expected,int events=1) {
        var api=new ControlledKernelApi { Fault=fault,EventCount=events };
        var buffers=new ControlledKernelBuffers();
        var result=CovenKernelReader.Collect(api,buffers);
        Require(result.Operation.ErrorClass==expected,"Unexpected operation class: "+fault+" actual="+result.Operation.ErrorClass);
        Require(result.Cleanup.AttemptedHandles==api.Closed.Count,"Missing cleanup observation: "+fault);
        Require(buffers.Owned.Count==0,"Private buffer leaked: "+fault);
        Require(api.Closed.Count==result.ObservedEventHandles+api.ParentHandles,"Owned handle leak: "+fault);
        if (result.ObservedEventHandles>0) Require(api.Closed[0]>=100,"Event not closed before parent: "+fault);
        if (api.Closed.Count>1) Require(api.Closed[api.Closed.Count-1]==1,"Publisher not closed last: "+fault);
        Count++;return result;
    }
    public static string Execute() {
        var ok=Run("","none");
        Require(ok.ProviderMatched && ok.EnumerationComplete && ok.Cleanup.Complete && ok.RetainedEventCount==1,"Success missing invariants");
        Require(ok.Events[0].KeywordsRawHex=="8000123456789abc" && ok.Events[0].KeywordsProviderHex=="0000123456789abc","Keyword reserved/provider mask lost");
        Require(ok.Events[0].Template.Status=="supported-flat" && ok.Events[0].Template.Fields.Count==1,"Template not projected");
        Run("stale-error","none");Run("smaller-used","none");
        Require(Run("","none",0).EnumerationComplete,"Empty enumeration not complete");
        foreach(string fault in new[]{"throw-open","throw-enum","throw-next","throw-property"}) {
            var failed=Run(fault,"collector-failed");Require(failed.Events.Count==0,"Unexpected exception exported definitions");
        }
        Run("open","provider-open-failed");Run("provider-property","property-probe-failed");Run("provider-mismatch","provider-mismatch");
        Run("enum-open","enumeration-open-failed");Run("next","enumeration-failed");Run("next-zero","enumeration-failed");
        Require(Run("probe-success","property-probe-failed").Operation.NativeError==null,"Successful probe exported native error");
        Require(Run("probe-success-stale","property-probe-failed").Operation.NativeError==null,"Successful probe exported stale native error");Run("probe-error","property-probe-failed");
        Run("small","property-size-limit");Run("large","property-size-limit");
        Run("read-error","property-read-failed");Run("growth","property-size-changed");Run("used-over","property-size-changed");Run("used-short","variant-invalid");
        foreach(string fault in new[]{"type","array","extra-type-flags","guid-short","guid-null","guid-unaligned","pointer-out","pointer-header","pointer-unaligned","pointer-negative","unterminated","payload-short"}) Run(fault,"variant-invalid");
        foreach(string fault in new[]{"range-id","range-version","range-channel","range-level","range-opcode","range-task"}) Run(fault,"descriptor-range");
        Require(Run("null-template","none").Events[0].Template.Status=="missing","Null template lost");
        Require(Run("private","none").Events[0].Template.Fields[0].KnownName==null,"Private field admitted");
        Run("template-limit","property-size-limit");
        Require(Run("total-limit","total-size-limit",256).PrivateBufferBytes<=1048576,"Cumulative limit crossed");
        Require(Run("global-fields","template-limit",256).TotalProjectedFields<=2048,"Global field limit crossed");
        Require(Run("","none",256).RetainedEventCount==256,"Exact event cap EOF failed");
        var cap=Run("","event-limit",257);Require(cap.ObservedEventHandles==257 && cap.RetainedEventCount==256 && !cap.EnumerationComplete && cap.Truncated,"Event cap probe lost");
        foreach(string fault in new[]{"close-event","close-enum","close-publisher","close-all","close-throw"}) {
            var closed=Run(fault,"none");Require(!closed.Cleanup.Complete && closed.Cleanup.FailedHandles>0,"Cleanup failure suppressed");
        }
        Require(Run("close-zero-then-error","none").Cleanup.FirstNativeError==null,"Later native code replaced zero first failure");
        Require(Run("close-error-then-throw","none").Cleanup.FirstNativeError==5,"Later exception replaced native first failure");
        foreach(string fault in new[]{"allocate-zero","allocate-throw"}) {
            var api=new ControlledKernelApi();var buffers=new ControlledKernelBuffers { Fault=fault };
            var failed=CovenKernelReader.Collect(api,buffers);
            Require(failed.Operation.ErrorClass=="collector-failed" && failed.Events.Count==0 && buffers.Owned.Count==0 && api.Closed.Count==1,"Allocation failure lost ownership");Count++;
        }
        Require(Run("close-throw-then-error","none").Cleanup.FirstNativeError==null,"Later native code replaced first cleanup failure");
        foreach(string fault in new[]{"before","after"}) {
            foreach(bool primary in new[]{false,true}) {
                var api=new ControlledKernelApi { Fault=primary?"type":"" };
                var buffers=new ControlledKernelBuffers { Fault=fault };
                var failed=CovenKernelReader.Collect(api,buffers);
                try {
                    Require(!failed.Cleanup.Complete && failed.Cleanup.FailedBuffers>0 && failed.Cleanup.AttemptedBuffers==buffers.FreeAttempts,"Buffer release failure hidden");
                    Require(failed.Operation.ErrorClass==(primary?"variant-invalid":"none"),"Buffer release replaced primary outcome");
                    Require(api.Closed.Count==api.ParentHandles+failed.ObservedEventHandles,"Buffer release skipped handle cleanup");
                } finally { foreach(var pointer in buffers.Owned) Marshal.FreeHGlobal(pointer);buffers.Owned.Clear(); }
                Count++;
            }
        }
        var both=Run("primary-and-cleanup","variant-invalid");Require(!both.Cleanup.Complete && both.Operation.FailedProperty==0,"Primary failure replaced by cleanup");
        return "provider reader controlled PASS cases="+Count+" (no native API invocation)";
    }
}
