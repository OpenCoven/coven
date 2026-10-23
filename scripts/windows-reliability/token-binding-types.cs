#nullable enable
using System;
using System.IO;
using System.Text;
using System.Text.Json;

// The fixed production entry has no injectable adapter or foreign-handle input.
// Collection remains synchronous on this calling OS thread.
public static class CovenOwnedTokenCollector {
    public static CovenTokenQueryResult Collect()=>CovenTokenQueryReader.Collect(new CovenTokenNativeApi());
}
public sealed class CovenTokenSourceFileIdentity {
    public string Id { get; }
    public string Sha256 { get; }
    public CovenTokenSourceFileIdentity(string id,string hash){Id=id;Sha256=hash;}
}
public sealed class CovenTokenSourceIdentity {
    public string Commit { get; }
    public CovenTokenSourceFileIdentity[] Files { get; }
    public bool BeforeAfterMatch { get; }
    public CovenTokenSourceIdentity(string commit,CovenTokenSourceFileIdentity[] files,bool matched){Commit=commit;Files=files;BeforeAfterMatch=matched;}
}
public sealed class CovenTokenAssemblyIdentity {
    public string Kind { get; }
    public string Sha256 { get; }
    public uint[] Version { get; }
    public CovenTokenAssemblyIdentity(string kind,string hash,uint[] version){Kind=kind;Sha256=hash;Version=version;}
}
public sealed class CovenTokenLibraryIdentity {
    public string Kind { get; }
    public string Sha256 { get; }
    public CovenTokenLibraryIdentity(string kind,string hash){Kind=kind;Sha256=hash;}
}
public sealed class CovenTokenHostIdentity {
    public string Kind=>"workflow-pwsh";
    public string AssemblyScope=>"selected-direct-managed-assemblies";
    public string ExecutableSha256 { get; }
    public CovenTokenAssemblyIdentity[] Assemblies { get; }
    public CovenTokenLibraryIdentity[] NativeLibraries { get; }
    public bool BeforeAfterMatch { get; }
    public CovenTokenHostIdentity(string executable,CovenTokenAssemblyIdentity[] assemblies,CovenTokenLibraryIdentity[] libraries,bool matched){ExecutableSha256=executable;Assemblies=assemblies;NativeLibraries=libraries;BeforeAfterMatch=matched;}
}
public sealed class CovenTokenProbeWire {
    public string Kind=>"coven.effective-token-probe.v1";
    public bool CaptureAttempted=>false;
    public CovenTokenSourceIdentity Source { get; }
    public CovenTokenHostIdentity Host { get; }
    public string Component { get; }
    internal CovenTokenProbeWire(CovenTokenSourceIdentity source,CovenTokenHostIdentity host,string component){Source=source;Host=host;Component=component;}
}
public static class CovenTokenBindingPublic {
    private static readonly JsonSerializerOptions Options=new(){PropertyNamingPolicy=JsonNamingPolicy.CamelCase};
    public static byte[] EncodeIdentity(object value) {
        if(value is null||(value.GetType()!=typeof(CovenTokenSourceIdentity)&&value.GetType()!=typeof(CovenTokenHostIdentity)))throw new ArgumentException("Token identity type rejected");
        using var stream=new CovenTokenCappedStream(8192);
        JsonSerializer.Serialize(stream,value,value.GetType(),Options);
        return stream.Complete();
    }
    public static CovenTokenResultEncoded EncodeProbe(CovenTokenQueryResult result,CovenTokenSourceIdentity source,CovenTokenHostIdentity host) {
        var component=CovenTokenResultPublic.Encode(result,new CovenTokenResultJsonSerializer());
        string text=new UTF8Encoding(false,true).GetString(component.Payload);
        using var stream=new CovenTokenCappedStream(32768);
        try {
            JsonSerializer.Serialize(stream,new CovenTokenProbeWire(source,host,text),Options);
            return new CovenTokenResultEncoded(stream.Complete(),component.ExitCode,component.DataWithheld);
        } catch { throw new IOException("Token probe output unavailable"); }
    }
}
