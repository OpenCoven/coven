---
title: "Owned token privilege projection implementation plan"
summary: "Portable first-component controls for the query-only token prerequisite."
read_when:
  - Implementing the bounded token diagnostic
source_adjacent_reason: "Records the first-component implementation and proof boundaries alongside its scripts."
---

# Owned Token Privilege Projection Implementation Plan

> **For agentic workers:** Use executing-plans to implement this component inline, with independent review before native integration.

**Goal:** Implement the pure bounded privilege projection needed by #1382, preserving its larger native/source/delivery acceptance and all #1227 causal requirements.

**Architecture:** One deterministic C# projector consumes a caller-bounded Windows ABI buffer and a private selected LUID. It returns five fixed booleans. Portable controlled cases compile in an isolated namespace; no native API is called.

**Tech Stack:** C#, PowerShell 7, System.Text.Json in controls.

## Task 1: Write the complete portable controls first

Create `scripts/windows-reliability/token-privileges-controls.cs` with:

```csharp
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
```

Create `scripts/windows-reliability/token-privileges-tests.ps1` with:

```powershell
$ErrorActionPreference='Stop'
$parts=foreach ($name in @('token-privileges.cs','token-privileges-controls.cs')) {
    $path=Join-Path $PSScriptRoot $name
    if (-not [IO.File]::Exists($path)) { throw 'Token privilege controlled source missing' }
    "namespace CovenTokenControlled {`n"+[IO.File]::ReadAllText($path,[Text.UTF8Encoding]::new($false,$true))+"`n}"
}
$references=@(Get-ChildItem -LiteralPath (Join-Path $PSHOME 'ref') -Filter '*.dll' | ForEach-Object FullName)
Add-Type -TypeDefinition ($parts -join "`n") -ReferencedAssemblies $references -ErrorAction Stop
$report=[CovenTokenControlled.CovenTokenPrivilegeControls]::Run()
if ($report.Failures.Length -ne 0) { throw ('Token privilege controlled failures: '+($report.Failures -join ',')) }
"token privilege controlled PASS cases=$($report.Cases) (no native API invocation)"
```

Run `pwsh -NoProfile -File scripts/windows-reliability/token-privileges-tests.ps1` before the projector exists. Retain the expected missing-feature setup failure separately; it is not a historical causal regression.

## Task 2: Implement the bounded projector

Create `scripts/windows-reliability/token-privileges.cs` with:

```csharp
#nullable enable
using System;
using System.Buffers.Binary;

public sealed class CovenTokenPrivilegeFacts {
    public bool Present { get; }
    public bool Enabled { get; }
    public bool EnabledByDefault { get; }
    public bool Removed { get; }
    public bool UsedForAccess { get; }
    internal CovenTokenPrivilegeFacts(bool present,uint attributes) {
        Present=present;
        Enabled=present && (attributes&2u)!=0;
        EnabledByDefault=present && (attributes&1u)!=0;
        Removed=present && (attributes&4u)!=0;
        UsedForAccess=present && (attributes&0x80000000u)!=0;
    }
}
// Only fixed facts leave this projection; token LUIDs and buffers stay private.
public static class CovenTokenPrivilegeProjector {
    private const int MaximumBytes=4096;
    private const uint MaximumCount=128;
    private const uint KnownAttributes=0x80000007u;
    public static CovenTokenPrivilegeFacts Project(byte[] buffer,int used,uint selectedLow,int selectedHigh) {
        if (buffer is null || buffer.Length>MaximumBytes || used<4 || used>buffer.Length)
            throw new ArgumentException("Invalid bounded token privilege buffer");
        uint count=BinaryPrimitives.ReadUInt32LittleEndian(buffer.AsSpan(0,4));
        if (count>MaximumCount || checked(4+(int)count*12)>used)
            throw new ArgumentException("Invalid bounded token privilege buffer");
        bool found=false;
        uint selectedAttributes=0;
        for (int index=0;index<(int)count;index++) {
            int offset=checked(4+index*12);
            uint low=BinaryPrimitives.ReadUInt32LittleEndian(buffer.AsSpan(offset,4));
            int high=BinaryPrimitives.ReadInt32LittleEndian(buffer.AsSpan(offset+4,4));
            if (low!=selectedLow || high!=selectedHigh) continue;
            if (found) throw new InvalidOperationException("Ambiguous token privilege");
            uint attributes=BinaryPrimitives.ReadUInt32LittleEndian(buffer.AsSpan(offset+8,4));
            if ((attributes&~KnownAttributes)!=0) throw new NotSupportedException("Token privilege attributes unsupported");
            found=true;
            selectedAttributes=attributes;
        }
        return new CovenTokenPrivilegeFacts(found,selectedAttributes);
    }
}
```

Run the same control command. Expect all25 cases to pass with no native invocation. Inspect actual count and logs; do not treat compilation as behavior proof.

## Task 3: Prove the controls discriminate and retain limits

Copy the three component files to an owned outside-checkout fixture directory. Replace only the Project method implementation with `return new CovenTokenPrivilegeFacts(false,0);` while keeping its signature/DTO. Run the identical copied harness: expect controlled failures for selected flags, duplicates, bounds and unsupported attributes. Keep this as an intentionally invalid controlled mutation, not old production behavior. Restore nothing in the real source because it was never changed.

Run `git diff --check`. Inspect public DTO reflection and closed failure-case output. Obtain independent component source review before the native adapter/reader is implemented. Keep all source uncommitted until full #1382 applicable gates/native pipeline/delivery are ready; no partial component closes #1382 or #1227.

Native query/cleanup/source/runtime/wire/publication/workflow components remain explicitly pending in #1382 and the private execution ledger. This plan completes only a testable pure component, not the issue outcome. Integrate against verified fetched main after #1379/#1380 delivery and refresh every affected gate before any signed commit.
