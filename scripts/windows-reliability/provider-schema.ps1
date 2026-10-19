function ConvertTo-CovenKernelFileSchema {
    param([Parameter(Mandatory)][string]$Xml)
    $result=[pscustomobject]@{providerMatched=$false;parseFailed=$false;events=@();unsupportedEvents=0;schemaComplete=$false}
    $digest={param([string]$s) [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($s))).ToLowerInvariant()}
    $reader=$null
    try {
        $settings=[Xml.XmlReaderSettings]::new()
        $settings.DtdProcessing=[Xml.DtdProcessing]::Prohibit
        $settings.XmlResolver=$null
        $settings.MaxCharactersInDocument=1048576
        $reader=[Xml.XmlReader]::Create([IO.StringReader]::new($Xml),$settings)
        $doc=[Xml.XmlDocument]::new();$doc.XmlResolver=$null;$doc.Load($reader)
        $root=$doc.DocumentElement
        if ($root.LocalName -ne 'publisherMetadata' -or
            $root.GetAttribute('name') -cne 'Microsoft-Windows-Kernel-File' -or
            $root.GetAttribute('guid').Trim('{}') -ine 'edd08927-9cc4-4e65-b970-c2560fb5c289') { return $result }
        $result.providerMatched=$true
        $events=[Collections.Generic.List[object]]::new()
        foreach ($e in $root.SelectNodes('./*[local-name()="events"]/*[local-name()="event"]')) {
            $id=0;$version=0
            if (-not [int]::TryParse($e.GetAttribute('value'),[ref]$id) -or $id -lt 0 -or $id -gt 65535 -or
                -not [int]::TryParse($e.GetAttribute('version'),[ref]$version) -or $version -lt 0 -or $version -gt 255) { $result.unsupportedEvents++;continue }
            $fields=[Collections.Generic.List[object]]::new()
            foreach ($data in $e.SelectNodes('.//*[local-name()="template"]//*[local-name()="data"]')) {
                $name=$data.GetAttribute('name');$type=$data.GetAttribute('inType')
                $knownName=$name -cin @('Irp','IrpPtr','FileObject','FileKey','IssuingThreadId','ThreadId','Status','ExtraInfo','InfoClass','FileName','FilePath','CreateOptions','CreateAttributes','ShareAccess','IoSize','IoFlags','ByteOffset','TTID')
                $knownType=$type -cin @('win:Pointer','win:UInt8','win:UInt16','win:UInt32','win:UInt64','win:Int8','win:Int16','win:Int32','win:Int64','win:UnicodeString','win:AnsiString','win:HexInt32','win:HexInt64','win:Binary')
                $fields.Add([pscustomobject]@{name=$(if($knownName){$name}else{$null});nameSha256=(& $digest $name);inType=$(if($knownType){$type}else{$null});typeSha256=(& $digest $type)})
            }
            $events.Add([pscustomobject]@{id=$id;version=$version;fields=$fields.ToArray();templateFieldsFound=($fields.Count -gt 0)})
        }
        $result.events=$events.ToArray()
        # Only structural extraction is established here; masks, templates and versions
        # still require comparison with actual installed metadata before capture.
        return $result
    } catch { $result.parseFailed=$true;return $result } finally { if ($reader) { $reader.Dispose() } }
}
