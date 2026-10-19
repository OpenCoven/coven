#nullable enable
using System;
using System.IO;
using System.Text.Json;

public sealed class CovenKernelCappedStream : Stream {
    private readonly byte[] bytes;
    private int used;
    public bool LimitReached { get; private set; }
    public CovenKernelCappedStream(int capacity) {
        if (capacity<1 || capacity>524288) throw new ArgumentOutOfRangeException(nameof(capacity));
        bytes=new byte[capacity];
    }
    public override bool CanRead { get { return false; } }
    public override bool CanSeek { get { return false; } }
    public override bool CanWrite { get { return true; } }
    public override long Length { get { return used; } }
    public override long Position { get { return used; } set { throw new NotSupportedException(); } }
    public override void Flush() { }
    public override int Read(byte[] buffer,int offset,int count) { throw new NotSupportedException(); }
    public override long Seek(long offset,SeekOrigin origin) { throw new NotSupportedException(); }
    public override void SetLength(long length) { throw new NotSupportedException(); }
    public override void Write(byte[] buffer,int offset,int count) {
        if (offset<0 || count<0 || offset>buffer.Length || count>buffer.Length-offset) throw new ArgumentOutOfRangeException();
        Write(buffer.AsSpan(offset,count));
    }
    public override void Write(ReadOnlySpan<byte> buffer) {
        if (buffer.Length>bytes.Length-used) { LimitReached=true;throw new IOException("Bounded collector output limit"); }
        buffer.CopyTo(bytes.AsSpan(used));used+=buffer.Length;
    }
    public override void WriteByte(byte value) {
        if (used==bytes.Length) { LimitReached=true;throw new IOException("Bounded collector output limit"); }
        bytes[used++]=value;
    }
    public byte[] Complete() { return bytes.AsSpan(0,used).ToArray(); }
}
public interface ICovenKernelSerializer { void Serialize(Stream stream,object value); }
public sealed class CovenKernelJsonSerializer : ICovenKernelSerializer {
    private static readonly JsonSerializerOptions Options=new JsonSerializerOptions { PropertyNamingPolicy=JsonNamingPolicy.CamelCase };
    public void Serialize(Stream stream,object value) { JsonSerializer.Serialize(stream,value,value.GetType(),Options); }
}
public sealed class CovenKernelCollectorSummary {
    public bool ProviderMatched { get; }
    public bool EnumerationComplete { get; }
    public int ObservedEventHandles { get; }
    public int RetainedEventCount { get; }
    public int PrivateBufferBytes { get; }
    public int TotalProjectedFields { get; }
    public bool Truncated { get; }
    public CovenKernelOperation Operation { get; }
    public CovenKernelCleanup Cleanup { get; }
    public CovenKernelCollectorSummary(CovenKernelCollector value) {
        ProviderMatched=value.ProviderMatched;EnumerationComplete=value.EnumerationComplete;
        ObservedEventHandles=value.ObservedEventHandles;RetainedEventCount=value.RetainedEventCount;
        PrivateBufferBytes=value.PrivateBufferBytes;TotalProjectedFields=value.TotalProjectedFields;
        Truncated=value.Truncated;Operation=value.Operation;Cleanup=value.Cleanup;
    }
}
public sealed class CovenKernelExportFailure {
    public string Kind { get { return "coven.kernel-file-collector-export-failure.v1"; } }
    public bool CaptureAttempted { get { return false; } }
    public bool MetadataComplete { get { return false; } }
    public bool CollectorDataWithheld { get { return true; } }
    public string ExportError { get; }
    public CovenKernelCollectorSummary CollectorSummary { get; }
    public CovenKernelExportFailure(string error,CovenKernelCollector value) {
        ExportError=error;CollectorSummary=new CovenKernelCollectorSummary(value);
    }
}
public sealed class CovenKernelEncoded {
    public byte[] Payload { get; }
    public int ExitCode { get; }
    public bool DataWithheld { get; }
    public string ErrorClass { get; }
    public CovenKernelEncoded(byte[] payload,int code,bool withheld,string error) {
        Payload=payload;ExitCode=code;DataWithheld=withheld;ErrorClass=error;
    }
}
public sealed class CovenKernelSourceFile {
    public string Id { get; }
    public string Sha256 { get; }
    public CovenKernelSourceFile(string id,string hash) { Id=id;Sha256=hash; }
}
public sealed class CovenKernelSource {
    public string Commit { get; }
    public System.Collections.Generic.List<CovenKernelSourceFile> Files { get; }
    public bool BeforeAfterMatch { get; }
    public CovenKernelSource(string commit,System.Collections.Generic.List<CovenKernelSourceFile> files,bool matched) {
        Commit=commit;Files=files;BeforeAfterMatch=matched;
    }
}
public sealed class CovenKernelAssembly {
    public string Kind { get; }
    public string Sha256 { get; }
    public uint[] Version { get; }
    public CovenKernelAssembly(string kind,string hash,uint[] version) { Kind=kind;Sha256=hash;Version=version; }
}
public sealed class CovenKernelHost {
    public string Kind { get { return "workflow-pwsh"; } }
    public string AssemblyScope { get { return "selected-direct-managed-assemblies"; } }
    public string ExecutableSha256 { get; }
    public System.Collections.Generic.List<CovenKernelAssembly> Assemblies { get; }
    public string NativeLibraryKind { get { return "wevtapi-system32"; } }
    public string NativeLibrarySha256 { get; }
    public bool BeforeAfterMatch { get; }
    public CovenKernelHost(string executable,System.Collections.Generic.List<CovenKernelAssembly> assemblies,string library,bool matched) {
        ExecutableSha256=executable;Assemblies=assemblies;NativeLibrarySha256=library;BeforeAfterMatch=matched;
    }
}
public sealed class CovenKernelProbeWire {
    public string SchemaVersion { get { return "coven.kernel-file-probe.v1"; } }
    public bool CaptureAttempted { get { return false; } }
    public bool MetadataComplete { get { return false; } }
    public CovenKernelSource Source { get; }
    public CovenKernelHost Host { get; }
    public object Collector { get; }
    public CovenKernelProbeWire(CovenKernelSource source,CovenKernelHost host,object collector) { Source=source;Host=host;Collector=collector; }
}
public static class CovenKernelPublic {
    private static int CollectionExit(CovenKernelCollector value) {
        return value.Operation.ErrorClass=="collector-failed"?1:
            value.Operation.ErrorClass=="none" && value.Cleanup.Complete && value.ProviderMatched && value.EnumerationComplete?0:2;
    }
    public static CovenKernelEncoded Encode(CovenKernelCollector value,ICovenKernelSerializer serializer) {
        return EncodeWrapped(value,CollectionExit(value),error=>new CovenKernelExportFailure(error,value),serializer);
    }
    public static CovenKernelEncoded EncodeProbe(CovenKernelCollector value,CovenKernelSource source,CovenKernelHost host,ICovenKernelSerializer serializer) {
        return EncodeWrapped(new CovenKernelProbeWire(source,host,value),CollectionExit(value),
            error=>new CovenKernelProbeWire(source,host,new CovenKernelExportFailure(error,value)),serializer);
    }
    private static CovenKernelEncoded EncodeWrapped(object normal,int normalExit,Func<string,object> fallback,ICovenKernelSerializer serializer) {
        string failure;
        using(var sink=new CovenKernelCappedStream(524288)) {
            try {
                serializer.Serialize(sink,normal);
                return new CovenKernelEncoded(sink.Complete(),normalExit,false,"none");
            } catch { failure=sink.LimitReached?"public-size-limit":"encoding-failed"; }
        }
        // Discard the private incomplete prefix. The original operation is never rewritten.
        using(var sink=new CovenKernelCappedStream(8192)) {
            try {
                serializer.Serialize(sink,fallback(failure));
                return new CovenKernelEncoded(sink.Complete(),failure=="public-size-limit"?2:1,true,failure);
            } catch {
                // Caller records a fixed source/outcome failure; no JSON prefix is emitted.
                return new CovenKernelEncoded(Array.Empty<byte>(),1,true,"encoding-failed");
            }
        }
    }
}
