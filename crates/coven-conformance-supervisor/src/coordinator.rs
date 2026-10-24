//! Standalone helper lifecycle. Every blocking stream operation has a worker.
use crate::{
    control::{self, Refusal},
    image::native::LockedImage,
    job::ContainedProcess,
    lease::native::{LeaseStatus, LeaseWatch},
    pipe::CancellablePipe,
    protocol::{self, Launch},
    worker::Workers,
    Failure,
};
use std::{
    ffi::OsString,
    fs::File,
    io::{BufReader, Write},
    os::windows::io::{AsHandle, FromRawHandle},
    ptr::{null, null_mut},
    sync::mpsc::{Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Pipes::CreatePipe;

type Header = (Launch, BufReader<CancellablePipe>);
struct Prepared {
    image: LockedImage,
    process: ContainedProcess,
    input: File,
    output: File,
    error: File,
}
fn refusal(error: Failure) -> Refusal {
    match error {
        Failure::InvalidLaunch => Refusal::InvalidLaunch,
        Failure::ContainmentUnavailable => Refusal::ContainmentUnavailable,
        Failure::CleanupUnproven => Refusal::CleanupUnproven,
    }
}
fn pipe() -> Result<(File, File), Refusal> {
    let (mut read, mut write) = (null_mut(), null_mut());
    // SAFETY: Writable outputs and default noninheritable byte-pipe handles.
    if unsafe { CreatePipe(&mut read, &mut write, null(), 4096) } == 0 {
        return Err(Refusal::IoRefused);
    }
    // SAFETY: Successful creation returned two independently owned handles.
    Ok(unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) })
}
fn prepare(
    program: OsString,
    cwd: OsString,
    args: Vec<String>,
    digest: String,
) -> Result<Prepared, Refusal> {
    let image = LockedImage::open(&program, &digest).map_err(refusal)?;
    let (target_input, input) = pipe()?;
    let (output, target_output) = pipe()?;
    let (error, target_error) = pipe()?;
    let process = image
        .spawn(
            &args,
            &cwd,
            [
                target_input.as_handle(),
                target_output.as_handle(),
                target_error.as_handle(),
            ],
        )
        .map_err(refusal)?;
    Ok(Prepared {
        image,
        process,
        input,
        output,
        error,
    })
}
fn poll<T>(receive: &Receiver<Result<T, Refusal>>) -> Result<Option<T>, Refusal> {
    match receive.try_recv() {
        Ok(value) => value.map(Some),
        Err(TryRecvError::Empty) => Ok(None),
        Err(TryRecvError::Disconnected) => Err(Refusal::IoRefused),
    }
}
struct Session {
    workers: Workers,
    started: Instant,
    timeout: Duration,
    nonce: Option<String>,
    header: Option<Receiver<Result<Header, Refusal>>>,
    preparing: Option<Receiver<Result<Prepared, Refusal>>>,
    process: Option<ContainedProcess>,
    image: Option<LockedImage>,
    lease: Option<LeaseWatch>,
    control: File,
}
impl Session {
    fn check(&self) -> Result<(), Refusal> {
        if self
            .lease
            .as_ref()
            .is_some_and(|lease| lease.status() == LeaseStatus::Failed)
        {
            return Err(Refusal::ControllerLost);
        }
        if self.started.elapsed() >= self.timeout {
            return Err(Refusal::DeadlineExceeded);
        }
        Ok(())
    }
    fn wait<T>(&self, receive: &Receiver<Result<T, Refusal>>) -> Result<T, Refusal> {
        loop {
            self.check()?;
            if let Some(value) = poll(receive)? {
                return Ok(value);
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn execute(
        &mut self,
        program: OsString,
        cwd: OsString,
        input: File,
        output: File,
        error: File,
    ) -> Result<u32, Refusal> {
        self.header = Some(self.workers.spawn(move |stop| {
            let mut reader = BufReader::new(CancellablePipe::new(input, stop).map_err(refusal)?);
            let launch = protocol::read_launch(&mut reader).map_err(refusal)?;
            Ok((launch, reader))
        })?);
        let (launch, mut reader) = self.wait(self.header.as_ref().ok_or(Refusal::IoRefused)?)?;
        self.header = None;
        self.nonce = Some(launch.nonce.clone());
        self.timeout = Duration::from_millis(launch.timeout_ms.into());
        self.check()?;
        let args = launch.args;
        self.preparing = Some(
            self.workers
                .spawn(move |_| prepare(program, cwd, args, launch.subject_sha256))?,
        );
        let prepared = self.wait(self.preparing.as_ref().ok_or(Refusal::IoRefused)?)?;
        self.preparing = None;
        self.process = Some(prepared.process);
        self.image = Some(prepared.image);
        self.check()?;
        let ready = control::ready(&launch.nonce).map_err(refusal)?;
        let control_file = self.control.try_clone().map_err(|_| Refusal::IoRefused)?;
        let ready_written = self.workers.spawn(move |stop| {
            let mut writer = CancellablePipe::new(control_file, stop).map_err(refusal)?;
            writer
                .write_all(&ready)
                .map_err(|_| Refusal::ControllerLost)
        })?;
        self.wait(&ready_written)?;
        // Never start a synchronous control read before the ready write settles.
        self.lease = Some(
            LeaseWatch::start(
                self.control.try_clone().map_err(|_| Refusal::IoRefused)?,
                launch.nonce,
            )
            .map_err(refusal)?,
        );
        loop {
            self.check()?;
            if self
                .lease
                .as_ref()
                .is_some_and(|watch| watch.status() == LeaseStatus::Acknowledged)
            {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let body = self.workers.spawn(move |stop| {
            let mut writer = CancellablePipe::new(prepared.input, stop).map_err(refusal)?;
            protocol::forward_input(&mut reader, &mut writer, launch.input_bytes).map_err(refusal)
        })?;
        let stdout = self.workers.spawn(move |stop| {
            let mut reader =
                CancellablePipe::new(prepared.output, stop.clone()).map_err(refusal)?;
            let mut writer = CancellablePipe::new(output, stop.clone()).map_err(refusal)?;
            crate::transfer::output(&mut reader, &mut writer, &stop)
        })?;
        let stderr = self.workers.spawn(move |stop| {
            let mut reader = CancellablePipe::new(prepared.error, stop.clone()).map_err(refusal)?;
            let mut writer = CancellablePipe::new(error, stop.clone()).map_err(refusal)?;
            crate::transfer::output(&mut reader, &mut writer, &stop)
        })?;
        self.check()?;
        self.process
            .as_mut()
            .ok_or(Refusal::ContainmentUnavailable)?
            .resume()
            .map_err(refusal)?;
        let mut complete = [false; 3];
        let mut exit = None;
        loop {
            self.check()?;
            for (index, receive) in [&body, &stdout, &stderr].into_iter().enumerate() {
                if !complete[index] && poll(receive)?.is_some() {
                    complete[index] = true;
                }
            }
            if exit.is_none() {
                let process = self
                    .process
                    .as_ref()
                    .ok_or(Refusal::ContainmentUnavailable)?;
                if let Some(code) = process.wait_for_exit(0).map_err(refusal)? {
                    // Root exit cannot leave descendants holding pipes or running.
                    process.terminate_and_wait().map_err(refusal)?;
                    exit = Some(code);
                }
            }
            self.check()?;
            if complete.into_iter().all(|value| value) {
                if let Some(code) = exit {
                    return Ok(code);
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn cleanup(&mut self) -> Result<(), Refusal> {
        // Kill known target ownership before cancelling any stream worker.
        let mut failed = self
            .process
            .as_ref()
            .is_some_and(|process| process.terminate_and_wait().is_err());
        failed |= self.workers.stop().is_err();
        // A preparation result can own a still-suspended job in its one-slot
        // channel. Thread settlement alone never proves that job was released.
        if let Some(receive) = self.preparing.take() {
            match receive.try_recv() {
                Ok(Ok(prepared)) => {
                    failed |= prepared.process.terminate_and_wait().is_err();
                    drop(prepared);
                }
                Ok(Err(Refusal::CleanupUnproven)) => failed = true,
                // An ordinary error may already have been consumed by execute.
                Ok(Err(_)) | Err(_) => {}
            }
        }
        self.header = None;
        let mut controller_lost = false;
        if let Some(lease) = self.lease.take() {
            match lease.stop_observed() {
                Ok(LeaseStatus::Failed) => controller_lost = true,
                Ok(_) => {}
                Err(_) => failed = true,
            }
        }
        self.process = None;
        self.image = None;
        if failed {
            Err(Refusal::CleanupUnproven)
        } else if controller_lost {
            Err(Refusal::ControllerLost)
        } else {
            Ok(())
        }
    }
}

/// Run only in the standalone helper: exit the process immediately on return,
/// especially status 3, since detached workers retain storage until process exit.
pub fn run(program: OsString, cwd: OsString, [input, output, error, control]: [File; 4]) -> u8 {
    let mut session = Session {
        workers: Workers::new(),
        started: Instant::now(),
        timeout: Duration::from_secs(10),
        nonce: None,
        header: None,
        preparing: None,
        process: None,
        image: None,
        lease: None,
        control,
    };
    let mut outcome = session.execute(program, cwd, input, output, error);
    if let Err(error) = session.cleanup() {
        if outcome != Err(Refusal::CleanupUnproven) {
            outcome = Err(error);
        }
    }
    // A preparation error or detached worker can leave ownership unproven.
    // Never race remaining I/O with another control write; exit immediately.
    if outcome == Err(Refusal::CleanupUnproven) {
        return 3;
    }
    if outcome.is_ok() && session.started.elapsed() >= session.timeout {
        outcome = Err(Refusal::DeadlineExceeded);
    }
    let status = match outcome {
        Ok(_) => 0,
        Err(Refusal::CleanupUnproven) => 3,
        Err(_) => 2,
    };
    let Some(nonce) = session.nonce.as_ref() else {
        return status;
    };
    let frame = match outcome {
        Ok(code) => control::exited(nonce, code),
        Err(error) => control::refused(nonce, error),
    };
    let Ok(frame) = frame else {
        return 2;
    };
    // The lease reader is now stopped. Keep terminal write supervision separate
    // from the already-stopped work group, with a fixed cooperative exit guard.
    let mut terminal = Workers::new();
    let file = session.control;
    let sent = terminal.spawn(move |stop| {
        let mut writer = CancellablePipe::new(file, stop).map_err(refusal)?;
        writer
            .write_all(&frame)
            .map_err(|_| Refusal::ControllerLost)
    });
    let started = Instant::now();
    let written = sent.and_then(|receive| loop {
        if started.elapsed() >= Duration::from_secs(10) {
            break Err(Refusal::DeadlineExceeded);
        }
        match poll(&receive) {
            Ok(Some(())) => break Ok(()),
            Err(error) => break Err(error),
            Ok(None) => thread::sleep(Duration::from_millis(5)),
        }
    });
    if terminal.stop().is_err() {
        return 3;
    }
    if written.is_err() && status == 0 {
        return 2;
    }
    status
}
