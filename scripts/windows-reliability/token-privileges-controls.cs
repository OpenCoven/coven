#nullable enable
using System;
using System.Buffers.Binary;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Text;
using System.Text.Json;

public sealed class CovenTokenPrivilegeControlReport {
    public int Cases { get; }
    public string[] Failures { get; }
    public CovenTokenPrivilegeControlReport(int cases,string[] failures) { Cases=cases;Failures=failures; }
}
public static class CovenTokenPrivilegeControls {
    private const uint SelectedLow=0x10203040;
    private const int SelectedHigh=-1234567;
    private static byte[] Buffer(params (uint Low,int High,uint Attributes)[] entries) {
        var result=new byte[checked(4+entries.Length*12)];
        BinaryPrimitives.WriteUInt32LittleEndian(result.AsSpan(0,4),(uint)entries.Length);
        for (int i=0;i<entries.Length;i++) {
            int offset=4+i*12;
            BinaryPrimitives.WriteUInt32LittleEndian(result.AsSpan(offset,4),entries[i].Low);
            BinaryPrimitives.WriteInt32LittleEndian(result.AsSpan(offset+4,4),entries[i].High);
            BinaryPrimitives.WriteUInt32LittleEndian(result.AsSpan(offset+8,4),entries[i].Attributes);
        }
        return result;
    }
    private static CovenTokenPrivilegeFacts Project(byte[] bytes) {
        return CovenTokenPrivilegeProjector.Project(bytes,bytes.Length,SelectedLow,SelectedHigh);
    }
    private static void Require(bool condition) {
        if (!condition) throw new InvalidOperationException("Controlled assertion failed");
    }
    private static void Flags(CovenTokenPrivilegeFacts facts,bool present,bool enabled=false,bool byDefault=false,bool removed=false,bool used=false) {
        Require(facts.Present==present);
        Require(facts.Enabled==enabled);
        Require(facts.EnabledByDefault==byDefault);
        Require(facts.Removed==removed);
        Require(facts.UsedForAccess==used);
    }
    private static void Reject(byte[] bytes,int used,Type type) {
        try { CovenTokenPrivilegeProjector.Project(bytes,used,SelectedLow,SelectedHigh); }
        catch (Exception error) {
            Require(error.GetType()==type);
            Require(error.Message is "Invalid bounded token privilege buffer" or "Ambiguous token privilege" or "Token privilege attributes unsupported");
            return;
        }
        throw new InvalidOperationException("Invalid input admitted");
    }
    public static CovenTokenPrivilegeControlReport Run() {
        int cases=0;
        var failures=new List<string>();
        void Case(string name,Action action) {
            cases++;
            try { action(); } catch { failures.Add(name); }
        }
        Case("empty-set",()=>Flags(Project(Buffer()),false));
        var flagCases=new (string Name,uint Attributes,bool Enabled,bool Default,bool Removed,bool Used)[] {
            ("disabled",0,false,false,false,false),
            ("default-only",1,false,true,false,false),
            ("enabled-only",2,true,false,false,false),
            ("enabled-and-default",3,true,true,false,false),
            ("removed",4,false,false,true,false),
            ("used",0x80000000,false,false,false,true),
            ("all-known",0x80000007,true,true,true,true)
        };
        foreach (var entry in flagCases) {
            var observed=entry;
            Case("selected-"+observed.Name,()=>Flags(Project(Buffer((SelectedLow,SelectedHigh,observed.Attributes))),true,observed.Enabled,observed.Default,observed.Removed,observed.Used));
        }
        Case("different-low-word",()=>Flags(Project(Buffer((SelectedLow+1,SelectedHigh,3))),false));
        Case("different-high-word",()=>Flags(Project(Buffer((SelectedLow,SelectedHigh+1,3))),false));
        Case("unselected-private-attributes",()=>Flags(Project(Buffer((SelectedLow+1,SelectedHigh,0xffffffff),(SelectedLow,SelectedHigh,1))),true,false,true));
        Case("maximum-count-selected-last",()=> {
            var entries=Enumerable.Range(0,128).Select(i=>((uint)i,0,0u)).ToArray();
            entries[127]=(SelectedLow,SelectedHigh,2);
            Flags(Project(Buffer(entries)),true,true);
        });
        Case("private-trailing-padding",()=> {
            byte[] source=Buffer((SelectedLow,SelectedHigh,3));
            byte[] marker=Encoding.ASCII.GetBytes("PRIVATE_TOKEN_AUTH_SID_SENTINEL");
            byte[] padded=new byte[4096];source.CopyTo(padded,0);marker.CopyTo(padded,source.Length);
            var facts=Project(padded);Flags(facts,true,true,true);
            string wire=JsonSerializer.Serialize(facts,new JsonSerializerOptions { PropertyNamingPolicy=JsonNamingPolicy.CamelCase });
            Require(Encoding.UTF8.GetByteCount(wire)<=128 && !wire.Contains("PRIVATE_TOKEN",StringComparison.Ordinal));
            using var document=JsonDocument.Parse(wire);
            Require(document.RootElement.EnumerateObject().Select(p=>p.Name).OrderBy(x=>x,StringComparer.Ordinal).SequenceEqual(new[]{"enabled","enabledByDefault","present","removed","usedForAccess"}));
        });
        Case("caller-input-unchanged",()=> {
            byte[] bytes=Buffer((SelectedLow,SelectedHigh,3));byte[] before=(byte[])bytes.Clone();
            Project(bytes);Require(bytes.SequenceEqual(before));
        });
        Case("exact-public-facts-surface",()=> {
            Type type=typeof(CovenTokenPrivilegeFacts);
            var properties=type.GetProperties(BindingFlags.Public|BindingFlags.Instance);
            Require(type.IsSealed && type.GetFields(BindingFlags.Public|BindingFlags.Instance).Length==0);
            Require(properties.All(p=>p.PropertyType==typeof(bool) && !p.CanWrite));
            Require(properties.Select(p=>p.Name).OrderBy(x=>x,StringComparer.Ordinal).SequenceEqual(new[]{"Enabled","EnabledByDefault","Present","Removed","UsedForAccess"}));
        });
        Case("duplicate-selected",()=>Reject(Buffer((SelectedLow,SelectedHigh,1),(SelectedLow,SelectedHigh,2)),28,typeof(InvalidOperationException)));
        Case("unknown-selected-attributes",()=>Reject(Buffer((SelectedLow,SelectedHigh,8)),16,typeof(NotSupportedException)));
        Case("null-buffer",()=>Reject(null!,4,typeof(ArgumentException)));
        Case("negative-used",()=>Reject(Buffer(),-1,typeof(ArgumentException)));
        Case("header-truncated",()=>Reject(new byte[3],3,typeof(ArgumentException)));
        Case("used-over-allocation",()=>Reject(Buffer(),5,typeof(ArgumentException)));
        Case("allocation-over-cap",()=>Reject(new byte[4097],4,typeof(ArgumentException)));
        Case("count-over-cap",()=> { var entries=Enumerable.Range(0,129).Select(i=>((uint)i,0,0u)).ToArray();var bytes=Buffer(entries);Reject(bytes,bytes.Length,typeof(ArgumentException)); });
        Case("count-overflow",()=> { var bytes=Buffer();BinaryPrimitives.WriteUInt32LittleEndian(bytes,uint.MaxValue);Reject(bytes,4,typeof(ArgumentException)); });
        Case("record-overruns-used",()=>Reject(Buffer((SelectedLow,SelectedHigh,0)),15,typeof(ArgumentException)));
        return new CovenTokenPrivilegeControlReport(cases,failures.ToArray());
    }
}
