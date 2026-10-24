#nullable enable
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Security.Cryptography;
using System.Text;
using System.Xml;

public sealed class CovenKernelField {
    public int Ordinal { get; set; }
    public string? KnownName { get; set; }
    public string NameSha256 { get; set; } = "";
    public string? KnownType { get; set; }
    public string TypeSha256 { get; set; } = "";
    public int? LiteralCount { get; set; }
    public int? LiteralLength { get; set; }
    public bool Unsupported { get; set; }
}
public sealed class CovenKernelTemplateResult {
    public string Status { get; set; } = "missing";
    public string? Sha256 { get; set; }
    public int? Utf16CodeUnits { get; set; }
    public List<CovenKernelField> Fields { get; } = new List<CovenKernelField>();
    public int UnsupportedCount { get; set; }
    // Ordinal-sorted closed codes of the rejection branches that executed.
    // They name branch classes only: no field attribution, counts or values.
    public List<string> Reasons { get; } = new List<string>();
}
// Raw XML is local to this projector; no returned member retains it.
public static class CovenKernelTemplate {
    private sealed class LimitReached : Exception {
        public string Code { get; }
        public LimitReached(string code) { Code = code; }
    }
    private const string EventNamespace = "http://schemas.microsoft.com/win/2004/08/events";
    private static readonly UTF8Encoding Utf8 = new UTF8Encoding(false, true);
    private static readonly HashSet<string> Names = new HashSet<string>(StringComparer.Ordinal) {
        "Irp", "IrpPtr", "FileObject", "FileKey", "IssuingThreadId", "ThreadId", "Status", "ExtraInfo", "InfoClass",
        "FileName", "FilePath", "CreateOptions", "CreateAttributes", "ShareAccess", "IoSize", "IoFlags", "ByteOffset", "TTID"
    };
    private static readonly HashSet<string> Types = new HashSet<string>(StringComparer.Ordinal) {
        "win:Pointer", "win:UInt8", "win:UInt16", "win:UInt32", "win:UInt64", "win:Int8", "win:Int16", "win:Int32", "win:Int64",
        "win:UnicodeString", "win:AnsiString", "win:HexInt32", "win:HexInt64", "win:Binary"
    };
    private static string Digest(string text) {
        return Convert.ToHexString(SHA256.HashData(Utf8.GetBytes(text))).ToLowerInvariant();
    }
    private static bool NamespaceSupported(string value) { return value == "" || value == EventNamespace; }
    private static int? Cardinality(string? value, CovenKernelField field, SortedSet<string> reasons, string code) {
        if (value == null) return null;
        int parsed;
        if (int.TryParse(value, NumberStyles.None, CultureInfo.InvariantCulture, out parsed) && parsed >= 0 && parsed <= 65535) return parsed;
        field.Unsupported = true;
        reasons.Add(code);
        return null;
    }
    private static CovenKernelField Field(XmlReader reader, int ordinal, HashSet<string> seen, SortedSet<string> reasons) {
        string name = reader.GetAttribute("name") ?? "";
        string type = reader.GetAttribute("inType") ?? "";
        var field = new CovenKernelField {
            Ordinal = ordinal, KnownName = Names.Contains(name) ? name : null, NameSha256 = Digest(name),
            KnownType = Types.Contains(type) ? type : null, TypeSha256 = Digest(type)
        };
        string? winBinding = reader.LookupNamespace("win");
        bool conflictingBinding = winBinding != null && winBinding != "http://manifests.microsoft.com/win/2004/08/windows/events";
        if (field.KnownName == null) reasons.Add("field-name");
        if (field.KnownType == null) reasons.Add("field-type");
        // Read-only: the decision below still runs seen.Add only for a known name and type.
        if (field.KnownName != null && field.KnownType != null && seen.Contains(name)) reasons.Add("field-duplicate");
        field.Unsupported = field.KnownName == null || field.KnownType == null || !seen.Add(name) || conflictingBinding;
        if (conflictingBinding) { field.KnownType = null; reasons.Add("win-namespace"); }
        field.LiteralCount = Cardinality(reader.GetAttribute("count"), field, reasons, "field-count");
        field.LiteralLength = Cardinality(reader.GetAttribute("length"), field, reasons, "field-length");
        // Display/map and other unmodelled attributes are not a complete schema.
        if (reader.MoveToFirstAttribute()) {
            do {
                if (reader.Prefix == "xmlns" || reader.Name == "xmlns") continue;
                if (reader.NamespaceURI != "" || (reader.Name != "name" && reader.Name != "inType" && reader.Name != "count" && reader.Name != "length")) { field.Unsupported = true; reasons.Add("field-attribute"); }
            } while (reader.MoveToNextAttribute());
            reader.MoveToElement();
        }
        return field;
    }
    public static CovenKernelTemplateResult Project(string? xml, int remainingFields) {
        if (remainingFields < 0 || remainingFields > 2048) throw new ArgumentOutOfRangeException(nameof(remainingFields));
        var result = new CovenKernelTemplateResult();
        if (xml == null) return result;
        result.Utf16CodeUnits = xml.Length;
        if (xml.Length > 32768) { result.Status = "limit"; result.Reasons.Add("size-limit"); return result; }
        try { result.Sha256 = Digest(xml); }
        catch (EncoderFallbackException) { result.Status = "invalid"; result.Reasons.Add("utf16-invalid"); return result; }
        if (xml.Length == 0) { result.Status = "empty"; return result; }
        var settings = new XmlReaderSettings {
            DtdProcessing = DtdProcessing.Prohibit, XmlResolver = null,
            MaxCharactersInDocument = 32768, IgnoreComments = false, IgnoreProcessingInstructions = false
        };
        bool rootSupported = false;
        CovenKernelField? activeField = null;
        var seen = new HashSet<string>(StringComparer.Ordinal);
        var reasons = new SortedSet<string>(StringComparer.Ordinal);
        int nodes = 0;
        try {
            using (var text = new StringReader(xml))
            using (var reader = XmlReader.Create(text, settings)) {
                while (reader.Read()) {
                    if (++nodes > 2048) throw new LimitReached("node-limit");
                    if (reader.Depth > 16) throw new LimitReached("depth-limit");
                    if (reader.NodeType == XmlNodeType.Element) {
                        if (reader.Depth == 0) {
                            rootSupported = reader.LocalName == "template" && NamespaceSupported(reader.NamespaceURI);
                            if (!rootSupported) { result.UnsupportedCount++; reasons.Add(reader.LocalName == "template" ? "root-namespace" : "root-name"); }
                            if (reader.MoveToFirstAttribute()) {
                                bool bad = false;
                                do {
                                    if (reader.Prefix == "xmlns" || reader.Name == "xmlns") continue;
                                    if (reader.NamespaceURI != "" || reader.Name != "tid") bad = true;
                                } while (reader.MoveToNextAttribute());
                                reader.MoveToElement();
                                if (bad) { result.UnsupportedCount++; reasons.Add("root-attribute"); }
                            }
                        } else if (rootSupported && reader.Depth == 1 && reader.LocalName == "data" && NamespaceSupported(reader.NamespaceURI)) {
                            if (result.Fields.Count >= 128) throw new LimitReached("field-limit");
                            if (result.Fields.Count >= remainingFields) throw new LimitReached("field-budget");
                            var field = Field(reader, result.Fields.Count, seen, reasons);
                            result.Fields.Add(field);
                            if (field.Unsupported) result.UnsupportedCount++;
                            activeField = reader.IsEmptyElement ? null : field;
                        } else {
                            reasons.Add(!rootSupported ? "root-content" : reader.Depth > 1 ? "nested-element" : reader.LocalName == "data" ? "data-namespace" : "child-element");
                            if (activeField != null && !activeField.Unsupported) { activeField.Unsupported = true; result.UnsupportedCount++; }
                            else result.UnsupportedCount++;
                        }
                    } else if (reader.NodeType == XmlNodeType.EndElement && reader.Depth == 1) {
                        activeField = null;
                    } else if ((reader.NodeType == XmlNodeType.Text || reader.NodeType == XmlNodeType.CDATA) && !string.IsNullOrWhiteSpace(reader.Value)) {
                        if (activeField != null && !activeField.Unsupported) activeField.Unsupported = true;
                        result.UnsupportedCount++;
                        reasons.Add("text");
                    }
                }
            }
            result.Status = result.UnsupportedCount == 0 ? "supported-flat" : "unsupported";
        } catch (LimitReached limit) { result.Fields.Clear(); result.UnsupportedCount = 0; result.Status = "limit"; reasons.Clear(); reasons.Add(limit.Code); }
        catch (XmlException) { result.Fields.Clear(); result.UnsupportedCount = 0; result.Status = "invalid"; reasons.Clear(); reasons.Add("xml-invalid"); }
        result.Reasons.AddRange(reasons);
        return result;
    }
}
