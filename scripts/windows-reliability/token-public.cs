#nullable enable
using System;
using System.IO;
using System.Text.Json;

public sealed class CovenTokenCappedStream : Stream {
    private readonly byte[] bytes;
    private int used;
    public bool LimitReached { get; private set; }
    public CovenTokenCappedStream(int capacity){if(capacity<1||capacity>65536)throw new ArgumentOutOfRangeException(nameof(capacity));bytes=new byte[capacity];}
    public override bool CanRead=>false;
    public override bool CanSeek=>false;
    public override bool CanWrite=>true;
    public override long Length=>used;
    public override long Position { get=>used;set=>throw new NotSupportedException(); }
    public override void Flush(){}
    public override int Read(byte[] buffer,int offset,int count)=>throw new NotSupportedException();
    public override long Seek(long offset,SeekOrigin origin)=>throw new NotSupportedException();
    public override void SetLength(long value)=>throw new NotSupportedException();
    public override void Write(byte[] buffer,int offset,int count){if(offset<0||count<0||offset>buffer.Length||count>buffer.Length-offset)throw new ArgumentOutOfRangeException();Write(buffer.AsSpan(offset,count));}
    public override void Write(ReadOnlySpan<byte> buffer){if(buffer.Length>bytes.Length-used){LimitReached=true;throw new IOException("Token output limit");}buffer.CopyTo(bytes.AsSpan(used));used+=buffer.Length;}
    public override void WriteByte(byte value){if(used==bytes.Length){LimitReached=true;throw new IOException("Token output limit");}bytes[used++]=value;}
    public byte[] Complete()=>bytes.AsSpan(0,used).ToArray();
}
public sealed class CovenTokenResultWire {
    public string Kind=>"coven.effective-token-result.v1";
    public bool CaptureAttempted=>false;
    public string PrivilegeName=>"SeSystemProfilePrivilege";
    public CovenTokenQueryResult Result { get; }
    internal CovenTokenResultWire(CovenTokenQueryResult result){Result=result;}
}
public sealed class CovenTokenResultSummary {
    public CovenTokenFailure Failure { get; }
    public uint NativeError { get; }
    public bool CleanupFailed { get; }
    public int ClearAttempts { get; }
    public int ClearFailures { get; }
    public int FreeAttempts { get; }
    public int FreeFailures { get; }
    public int CloseAttempts { get; }
    public int CloseFailures { get; }
    internal CovenTokenResultSummary(CovenTokenQueryResult result){Failure=result.Failure;NativeError=result.NativeError;CleanupFailed=result.CleanupFailed;ClearAttempts=result.ClearAttempts;ClearFailures=result.ClearFailures;FreeAttempts=result.FreeAttempts;FreeFailures=result.FreeFailures;CloseAttempts=result.CloseAttempts;CloseFailures=result.CloseFailures;}
}
public sealed class CovenTokenExportFailureWire {
    public string Kind=>"coven.effective-token-export-failure.v1";
    public bool CaptureAttempted=>false;
    public bool FactsWithheld=>true;
    public string ExportError { get; }
    public CovenTokenResultSummary ResultSummary { get; }
    internal CovenTokenExportFailureWire(string reason,CovenTokenQueryResult result){ExportError=reason;ResultSummary=new CovenTokenResultSummary(result);}
}
public interface ICovenTokenResultSerializer { void Serialize(Stream stream,CovenTokenResultWire value); }
internal interface ICovenTokenFallbackSerializer { void Serialize(Stream stream,CovenTokenExportFailureWire value); }
public sealed class CovenTokenResultJsonSerializer : ICovenTokenResultSerializer,ICovenTokenFallbackSerializer {
    private static readonly JsonSerializerOptions Options=new(){PropertyNamingPolicy=JsonNamingPolicy.CamelCase};
    public void Serialize(Stream stream,CovenTokenResultWire value)=>JsonSerializer.Serialize(stream,value,Options);
    void ICovenTokenFallbackSerializer.Serialize(Stream stream,CovenTokenExportFailureWire value)=>JsonSerializer.Serialize(stream,value,Options);
}
public sealed class CovenTokenResultEncoded {
    public byte[] Payload { get; }
    public int ExitCode { get; }
    public bool DataWithheld { get; }
    internal CovenTokenResultEncoded(byte[] payload,int exitCode,bool withheld){Payload=payload;ExitCode=exitCode;DataWithheld=withheld;}
}
public static class CovenTokenResultPublic {
    public static CovenTokenResultEncoded Encode(CovenTokenQueryResult result,ICovenTokenResultSerializer serializer)=>EncodeControlled(result,serializer,new CovenTokenResultJsonSerializer());
    // Controlled failure injection never changes the production fallback choice.
    internal static CovenTokenResultEncoded EncodeControlled(CovenTokenQueryResult result,ICovenTokenResultSerializer serializer,ICovenTokenFallbackSerializer fallback) {
        if(result is null)throw new ArgumentException("Token result missing");
        using var stream=new CovenTokenCappedStream(4096);
        try {
            serializer.Serialize(stream,new CovenTokenResultWire(result));
            byte[] payload=stream.Complete();if(payload.Length==0)throw new IOException("Token output empty");
            bool complete=result.Failure==CovenTokenFailure.None&&!result.CleanupFailed&&result.Facts is not null;
            return new CovenTokenResultEncoded(payload,complete?0:1,!complete);
        } catch {
            string reason=stream.LimitReached?"output-limit":"serialization-failed";
            try {
                using var recovery=new CovenTokenCappedStream(4096);
                fallback.Serialize(recovery,new CovenTokenExportFailureWire(reason,result));
                byte[] payload=recovery.Complete();if(payload.Length==0)throw new IOException("Token output empty");
                return new CovenTokenResultEncoded(payload,2,true);
            } catch {
                // No result object, prefix or original exception leaves this call.
                // The eventual child writes stdout only after successful return.
                throw new IOException("Token output unavailable");
            }
        }
    }
}
