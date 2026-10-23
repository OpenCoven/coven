#nullable enable
using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
using System.Text.Json;

internal sealed class ControlledTokenSerializer : ICovenTokenResultSerializer {
    internal bool Limit;
    public void Serialize(Stream stream,CovenTokenResultWire value) {
        if(Limit){stream.Write(new byte[4097]);return;}
        byte[] marker=Encoding.UTF8.GetBytes("PRIVATE_TOKEN_EXCEPTION_PATH_SENTINEL");stream.Write(marker);throw new InvalidOperationException("PRIVATE_TOKEN_EXCEPTION_PATH_SENTINEL");
    }
}
internal sealed class ControlledFallbackSerializer : ICovenTokenFallbackSerializer {
    internal bool Limit;
    public void Serialize(Stream stream,CovenTokenExportFailureWire value){
        if(Limit)stream.Write(new byte[4097]);
        else{stream.Write(Encoding.UTF8.GetBytes("PRIVATE_FALLBACK_SENTINEL"));throw new Exception("PRIVATE_FALLBACK_SENTINEL");}
    }
}
public sealed class CovenTokenPublicControlReport {
    public int Cases { get; }
    public string[] Failures { get; }
    public CovenTokenPublicControlReport(int cases,string[] failures){Cases=cases;Failures=failures;}
}
public static class CovenTokenPublicControls {
    private static void Require(bool value){if(!value)throw new InvalidOperationException("Controlled assertion failed");}
    private static CovenTokenQueryResult Success()=>new(CovenTokenFailure.None,0,6,0,6,0,2,0,new CovenEffectiveTokenFacts(CovenTokenContext.Process,1,null,3,false,new CovenTokenPrivilegeFacts(true,1)));
    private static CovenTokenQueryResult PrimaryCleanup()=>new(CovenTokenFailure.Read,5,6,1,6,0,2,1,null);
    public static byte[] Fixture(string name) {
        return name switch {
            "success"=>CovenTokenResultPublic.Encode(Success(),new CovenTokenResultJsonSerializer()).Payload,
            "primary-cleanup"=>CovenTokenResultPublic.Encode(PrimaryCleanup(),new CovenTokenResultJsonSerializer()).Payload,
            "output-limit"=>CovenTokenResultPublic.Encode(PrimaryCleanup(),new ControlledTokenSerializer{Limit=true}).Payload,
            "serialization-failed"=>CovenTokenResultPublic.Encode(PrimaryCleanup(),new ControlledTokenSerializer()).Payload,
            _=>throw new ArgumentException("Controlled fixture not found")
        };
    }
    public static CovenTokenPublicControlReport Run() {
        int cases=0;var failures=new List<string>();
        void Case(string name,Action action){cases++;try{action();}catch{failures.Add(name);}}
        Case("complete-camel-case-success",()=>{
            var value=CovenTokenResultPublic.Encode(Success(),new CovenTokenResultJsonSerializer());Require(value.ExitCode==0&&!value.DataWithheld&&value.Payload.Length<=4096);
            using var json=JsonDocument.Parse(value.Payload);var root=json.RootElement;
            Require(root.GetProperty("kind").GetString()=="coven.effective-token-result.v1"&&root.GetProperty("captureAttempted").ValueKind==JsonValueKind.False);
            Require(root.GetProperty("privilegeName").GetString()=="SeSystemProfilePrivilege");
            var result=root.GetProperty("result");Require(result.GetProperty("failure").GetInt32()==0&&result.GetProperty("nativeError").GetUInt32()==0);
            var facts=result.GetProperty("facts");Require(facts.GetProperty("context").GetInt32()==1&&facts.GetProperty("tokenType").GetInt32()==1&&facts.GetProperty("impersonationLevel").ValueKind==JsonValueKind.Null);
            Require(!facts.GetProperty("privilege").GetProperty("enabled").GetBoolean()&&facts.GetProperty("privilege").GetProperty("enabledByDefault").GetBoolean());
        });
        Case("primary-and-cleanup-preserved",()=>{
            var value=CovenTokenResultPublic.Encode(PrimaryCleanup(),new CovenTokenResultJsonSerializer());Require(value.ExitCode==1&&value.DataWithheld);
            using var json=JsonDocument.Parse(value.Payload);var result=json.RootElement.GetProperty("result");Require(result.GetProperty("failure").GetInt32()==7&&result.GetProperty("nativeError").GetUInt32()==5&&result.GetProperty("cleanupFailed").GetBoolean()&&result.GetProperty("facts").ValueKind==JsonValueKind.Null);
        });
        foreach(bool limit in new[]{false,true})Case(limit?"complete-limit-fallback":"complete-exception-fallback",()=>{
            var value=CovenTokenResultPublic.Encode(PrimaryCleanup(),new ControlledTokenSerializer{Limit=limit});Require(value.ExitCode==2&&value.DataWithheld&&value.Payload.Length<=4096);
            string text=new UTF8Encoding(false,true).GetString(value.Payload);Require(!text.Contains("PRIVATE_"));
            using var json=JsonDocument.Parse(value.Payload);var root=json.RootElement;
            Require(root.GetProperty("kind").GetString()=="coven.effective-token-export-failure.v1"&&root.GetProperty("factsWithheld").GetBoolean()&&root.GetProperty("exportError").GetString()==(limit?"output-limit":"serialization-failed"));
            var summary=root.GetProperty("resultSummary");Require(summary.GetProperty("failure").GetInt32()==7&&summary.GetProperty("nativeError").GetUInt32()==5&&summary.GetProperty("cleanupFailed").GetBoolean()&&!summary.TryGetProperty("facts",out _));
        });
        Case("exact-cap-stream",()=>{using var stream=new CovenTokenCappedStream(4096);stream.Write(new byte[4096]);Require(stream.Complete().Length==4096&&!stream.LimitReached);try{stream.WriteByte(1);}catch(IOException){Require(stream.LimitReached);return;}throw new Exception();});
        Case("atomic-stream-write",()=>{using var stream=new CovenTokenCappedStream(4);stream.WriteByte(3);try{stream.Write(new byte[4]);}catch(IOException){Require(stream.Complete().SequenceEqual(new byte[]{3}));return;}throw new Exception();});
        foreach(bool limit in new[]{false,true})Case(limit?"fallback-limit-zero-result":"fallback-exception-zero-result",()=>{
            CovenTokenResultEncoded? escaped=null;
            try{escaped=CovenTokenResultPublic.EncodeControlled(PrimaryCleanup(),new ControlledTokenSerializer(),new ControlledFallbackSerializer{Limit=limit});}
            catch(IOException error){Require(escaped is null&&error.Message=="Token output unavailable"&&error.InnerException is null);return;}
            throw new Exception();
        });
        Case("null-result-rejected",()=>{try{CovenTokenResultPublic.Encode(null!,new CovenTokenResultJsonSerializer());}catch(ArgumentException){return;}throw new Exception();});
        return new CovenTokenPublicControlReport(cases,failures.ToArray());
    }
}
