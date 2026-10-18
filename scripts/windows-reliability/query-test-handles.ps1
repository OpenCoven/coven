function Get-OwnedReaderHandle([IO.StreamReader]$Reader) {
    $stream=$Reader.BaseStream
    if ($stream -is [IO.FileStream]) { return $stream.SafeFileHandle }
    if ($stream -is [IO.Pipes.PipeStream]) { return $stream.SafePipeHandle }
    throw 'Unexpected redirected stream type in handle-closure test'
}
