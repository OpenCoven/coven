#nullable enable
using System;
using System.IO;
using System.Text;
using System.Text.Json;
public sealed class ControlledSerializer : ICovenKernelSerializer {
    public bool FirstThrows;
    public bool FallbackThrows;
    public bool PartialPrefix;
    public int Calls;
    public void Serialize(Stream stream,object value) {
        Calls++;
        if ((Calls==1 && FirstThrows) || (Calls==2 && FallbackThrows)) { if (PartialPrefix) stream.Write(Encoding.UTF8.GetBytes("PRIVATE_SENTINEL")); throw new Exception("PRIVATE_SENTINEL"); }
        new CovenKernelJsonSerializer().Serialize(stream,value);
    }
}
public static class ControlledPublicTests {
    private static void Check(bool value,string label) { if (!value) throw new Exception(label); }
    private static CovenKernelCollector Collect(int count,string fault="") { return CovenKernelReader.Collect(new ControlledKernelApi { EventCount=count,Fault=fault },new ControlledKernelBuffers()); }
    public static CovenKernelSource Source(string phaseHash) {
        var files=new System.Collections.Generic.List<CovenKernelSourceFile>();
        string[] ids="workflow runner probe binding binding-tests reader template public-encoder metadata-tests controlled-api template-tests adapter-tests encoder-tests controlled-encoder query-helper query-summary source-stability phase-validator validator validator-tests envelope-tests".Split(' ');
        Array.Sort(ids,StringComparer.Ordinal);
        foreach(string id in ids) files.Add(new CovenKernelSourceFile(id,id=="phase-validator"?phaseHash:new string('0',64)));
        return new CovenKernelSource(new string('1',40),files,true);
    }
    public static CovenKernelHost Host() {
        var assemblies=new System.Collections.Generic.List<CovenKernelAssembly>();
        foreach(string kind in new[]{"corelib","crypto","json","powershell","xml"}) assemblies.Add(new CovenKernelAssembly(kind,new string('3',64),new uint[]{kind=="powershell"?7u:8u,0,0,0}));
        return new CovenKernelHost(new string('2',64),assemblies,new string('4',64),true);
    }
    public static string ExecuteProbe() {
        foreach(var scenario in new[]{(Count:1,Fault:""),(Count:257,Fault:"global-fields"),(Count:257,Fault:"eight-fields"),(Count:256,Fault:"eight-fields")}) {
            var value=Collect(scenario.Count,scenario.Fault);
            var encoded=CovenKernelPublic.EncodeProbe(value,Source(new string('0',64)),Host(),new CovenKernelJsonSerializer());
            Check(encoded.Payload.Length>0 && encoded.Payload.Length<=524288,"Probe wire bound failed");
            using(var doc=JsonDocument.Parse(encoded.Payload)) {
                var wire=doc.RootElement;
                Check(wire.GetProperty("schemaVersion").GetString()=="coven.kernel-file-probe.v2","Probe version changed");
                Check(!wire.TryGetProperty("query",out _),"Private probe forged parent observation");
                Check(wire.GetProperty("source").GetProperty("files").GetArrayLength()==21,"Probe source omitted files");
                Check(wire.GetProperty("host").GetProperty("assemblies").GetArrayLength()==5,"Probe selected host omitted assemblies");
                if (encoded.DataWithheld) Check(wire.GetProperty("collector").GetProperty("collectorSummary").GetProperty("operation").GetProperty("errorClass").GetString()==value.Operation.ErrorClass,"Probe fallback replaced primary");
                else {
                    var collector=wire.GetProperty("collector");
                    Check(collector.GetProperty("operation").GetProperty("errorClass").GetString()==value.Operation.ErrorClass,"Probe collector changed");
                    foreach(var item in collector.GetProperty("events").EnumerateArray()) {
                        var template=item.GetProperty("template");
                        Check(template.GetProperty("reasons").ValueKind==JsonValueKind.Array,"Probe template omitted reasons");
                        Check(template.GetProperty("status").GetString()!="unsupported" || template.GetProperty("reasons").GetArrayLength()>0,"Unsupported template omitted its reasons");
                    }
                }
            }
            Check(encoded.ExitCode==(scenario.Count==1?0:2),"Probe exit suppressed failure");
        }
        return "bounded child probe-wire controls PASS (no native API invocation)";
    }
    public static string Execute() {
        var serializer=new CovenKernelJsonSerializer();
        var small=Collect(1);
        var normal=CovenKernelPublic.Encode(small,serializer);
        Check(normal.ExitCode==0 && !normal.DataWithheld && normal.Payload.Length<=524288,"Small collector did not round trip");
        using(var parsed=JsonDocument.Parse(normal.Payload)) Check(parsed.RootElement.GetProperty("retainedEventCount").GetInt32()==1,"Collector protocol changed");
        foreach(var scenario in new[]{(Count:257,Fault:"global-fields"),(Count:257,Fault:"eight-fields"),(Count:256,Fault:"eight-fields")}) {
            var value=Collect(scenario.Count,scenario.Fault);
            string primary=value.Operation.ErrorClass;
            var encoded=CovenKernelPublic.Encode(value,serializer);
            Check(encoded.DataWithheld && encoded.ExitCode==2 && encoded.Payload.Length<=8192,"Oversized component was emitted");
            using(var doc=JsonDocument.Parse(encoded.Payload)) {
                var root=doc.RootElement;
                Check(root.GetProperty("kind").GetString()=="coven.kernel-file-collector-export-failure.v1","Wrong failure protocol");
                Check(root.GetProperty("exportError").GetString()=="public-size-limit","Wrong export failure class");
                Check(root.GetProperty("collectorSummary").GetProperty("operation").GetProperty("errorClass").GetString()==primary,"Original operation replaced");
                Check(!root.TryGetProperty("events",out _),"Failure output retained events");
            }
        }
        var caught=CovenKernelPublic.Encode(small,new ControlledSerializer { FirstThrows=true,PartialPrefix=true });
        Check(caught.ExitCode==1 && caught.DataWithheld && caught.Payload.Length>0,"Encoding exception suppressed");
        Check(!Encoding.UTF8.GetString(caught.Payload).Contains("PRIVATE_SENTINEL"),"Encoding private text leaked");
        var unavailable=CovenKernelPublic.Encode(small,new ControlledSerializer { FirstThrows=true,FallbackThrows=true,PartialPrefix=true });
        Check(unavailable.ExitCode==1 && unavailable.DataWithheld && unavailable.Payload.Length==0 && unavailable.ErrorClass=="encoding-failed","Failed fallback emitted data");
        using(var sink=new CovenKernelCappedStream(524288)) {
            sink.Write(new byte[524288],0,524288);Check(sink.Length==524288,"Exact cap rejected");
            bool refused=false;try { sink.WriteByte(1); }catch { refused=true; }
            Check(refused && sink.LimitReached && sink.Length==524288,"Byte beyond cap written");
        }
        return "bounded collector encoder controlled PASS (no native API invocation)";
    }
}
