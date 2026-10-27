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
