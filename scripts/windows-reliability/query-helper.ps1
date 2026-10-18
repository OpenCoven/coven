# Publish only caller-approved metadata, never these raw stdout/stderr strings.
Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
public sealed class CovenQueryResult {
    public bool Started, TimedOut, OutputLimited, Reaped, RootExited, ReadersSettled;
    public int? ExitCode;
    public long ElapsedMs;
    public string Stdout = "", Stderr = "", Failure = "", CleanupFailure = "", ReaderFailure = "";
}
public static class CovenNativeQuery {
    private sealed class TextResult {
        public string Text = "", Failure = "";
        public bool Limited, Cancelled;
    }
    private static async Task<TextResult> ReadBounded(StreamReader reader, int limit, CancellationToken token) {
        var text = new StringBuilder();
        var buffer = new char[4096];
        try {
            while (true) {
                int count = await reader.ReadAsync(buffer.AsMemory(), token).ConfigureAwait(false);
                if (count == 0) return new TextResult { Text = text.ToString() };
                int available = limit - text.Length;
                text.Append(buffer, 0, Math.Min(count, available));
                if (count > available) return new TextResult { Text = text.ToString(), Limited = true };
            }
        } catch (OperationCanceledException) {
            return new TextResult { Text = text.ToString(), Cancelled = true };
        } catch (Exception error) {
            return new TextResult { Text = text.ToString(), Failure = error.GetType().Name };
        }
    }
    private static bool Settled(Task first, Task second) {
        return (first == null || first.IsCompleted) && (second == null || second.IsCompleted);
    }
    private static bool WaitReaders(Task first, Task second, int budgetMs) {
        var timer = Stopwatch.StartNew();
        while (!Settled(first, second) && timer.ElapsedMilliseconds < budgetMs) Thread.Sleep(10);
        return Settled(first, second);
    }
    private static bool ReaderFailed(Task<TextResult> task) {
        return task != null && (task.IsFaulted || task.IsCanceled ||
            (task.IsCompletedSuccessfully && task.Result.Failure != ""));
    }
    private static void Observe(Task<TextResult> task, CovenQueryResult result, bool stdout) {
        if (task == null) return;
        if (task.IsCompletedSuccessfully) {
            if (stdout) result.Stdout = task.Result.Text; else result.Stderr = task.Result.Text;
            result.OutputLimited |= task.Result.Limited;
            if (task.Result.Failure != "") result.ReaderFailure = task.Result.Failure;
        } else if (task.IsFaulted) {
            result.ReaderFailure = task.Exception.GetBaseException().GetType().Name;
        } else if (!task.IsCompleted) {
            // Observe a late fault even when a broken stream refuses to settle.
            task.ContinueWith(t => { var observed = t.Exception; },
                CancellationToken.None, TaskContinuationOptions.OnlyOnFaulted,
                TaskScheduler.Default);
        }
    }
    public static CovenQueryResult Run(string executable, string[] arguments, int timeoutMs, int limitChars) {
        return RunForTests(executable, arguments, timeoutMs, limitChars, null, null);
    }
    public static CovenQueryResult RunForTests(string executable, string[] arguments, int timeoutMs, int limitChars, string fault, Action<Process> started) {
        if (!Path.IsPathFullyQualified(executable) || timeoutMs < 1 || timeoutMs > 30000 || limitChars < 1 || limitChars > 524288)
            throw new ArgumentException("Invalid bounded query configuration");
        var result = new CovenQueryResult();
        var timer = Stopwatch.StartNew();
        using (var cancel = new CancellationTokenSource())
        using (var process = new Process()) {
            Task<TextResult> stdout = null, stderr = null;
            StreamReader stdoutReader = null, stderrReader = null;
            process.StartInfo = new ProcessStartInfo(executable) {
                UseShellExecute = false, RedirectStandardOutput = true,
                RedirectStandardError = true, RedirectStandardInput = true, CreateNoWindow = true
            };
            foreach (var argument in arguments) process.StartInfo.ArgumentList.Add(argument);
            try {
                result.Started = process.Start();
                if (!result.Started) throw new InvalidOperationException("Query not started");
                stdoutReader = process.StandardOutput; stderrReader = process.StandardError;
                if (started != null) started(process);
                if (fault == "after-start") throw new InvalidOperationException("Injected post-start failure");
                process.StandardInput.Close();
                stdout = fault == "reader" ? Task.FromException<TextResult>(new IOException("Injected reader failure"))
                    : ReadBounded(stdoutReader, limitChars, cancel.Token);
                stderr = ReadBounded(stderrReader, limitChars, cancel.Token);
                while (!process.HasExited) {
                    result.OutputLimited = (stdout.IsCompletedSuccessfully && stdout.Result.Limited) ||
                                           (stderr.IsCompletedSuccessfully && stderr.Result.Limited);
                    result.TimedOut = timer.ElapsedMilliseconds >= timeoutMs;
                    if (ReaderFailed(stdout) || ReaderFailed(stderr)) { result.Failure = "ReaderFailed"; break; }
                    if (result.OutputLimited || result.TimedOut) break;
                    process.WaitForExit(20);
                }
            } catch (Exception error) {
                result.Failure = error.GetType().Name;
            } finally {
                // Every post-start path reaches termination/root wait and reader settlement.
                if (result.Started) {
                    try { if (!process.HasExited) process.Kill(true); }
                    catch (Exception error) { result.CleanupFailure = error.GetType().Name; }
                    try {
                        result.RootExited = process.WaitForExit(3000);
                        if (result.RootExited) result.ExitCode = process.ExitCode;
                    } catch (Exception error) { result.CleanupFailure = error.GetType().Name; }
                    result.ReadersSettled = WaitReaders(stdout, stderr, 2000);
                    if (!result.ReadersSettled) {
                        if (result.Failure == "") result.Failure = "InheritedPipesRemainOpen";
                        try { cancel.Cancel(); }
                        catch (Exception error) { result.CleanupFailure = error.GetType().Name; }
                        try { stdoutReader?.Close(); }
                        catch (Exception error) { result.CleanupFailure = error.GetType().Name; }
                        try { stderrReader?.Close(); }
                        catch (Exception error) { result.CleanupFailure = error.GetType().Name; }
                        result.ReadersSettled = WaitReaders(stdout, stderr, 2000);
                    }
                } else { result.RootExited = true; result.ReadersSettled = true; }
                Observe(stdout, result, true); Observe(stderr, result, false);
                // Process.Dispose does not own externally accessed readers.
                try { stdoutReader?.Dispose(); }
                catch (Exception error) { result.CleanupFailure = error.GetType().Name; }
                try { stderrReader?.Dispose(); }
                catch (Exception error) { result.CleanupFailure = error.GetType().Name; }
                if (result.ReaderFailure != "" && result.Failure == "") result.Failure = "ReaderFailed";
                if (!result.ReadersSettled && result.CleanupFailure == "") result.CleanupFailure = "ReadersUnsettled";
                result.Reaped = result.RootExited && result.ReadersSettled;
                result.ElapsedMs = timer.ElapsedMilliseconds;
            }
        }
        return result;
    }
}
'@
