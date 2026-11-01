//! Bounded fragmentation inside the authenticated encrypted record stream.
use super::{Channel, Records, MAX_PLAINTEXT_BYTES};
use anyhow::{ensure, Result};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

impl<R: Records> Channel<R> {
    pub fn send_message(
        &mut self,
        message: &[u8],
        maximum: usize,
        deadline: Instant,
    ) -> Result<()> {
        let result = self.send_message_inner(message, maximum, deadline);
        if result.is_err() {
            self.cipher = None;
        }
        result
    }

    fn send_message_inner(
        &mut self,
        message: &[u8],
        maximum: usize,
        deadline: Instant,
    ) -> Result<()> {
        ensure!(
            message.len() <= maximum && message.len() <= u32::MAX as usize,
            "message exceeds limit"
        );
        let first = message.len().min(MAX_PLAINTEXT_BYTES - 5);
        let mut record = Zeroizing::new(Vec::with_capacity(MAX_PLAINTEXT_BYTES));
        record.push(1);
        record.extend_from_slice(&(message.len() as u32).to_be_bytes());
        record.extend_from_slice(&message[..first]);
        self.send(&record, deadline)?;
        for chunk in message[first..].chunks(MAX_PLAINTEXT_BYTES - 1) {
            record.clear();
            record.push(2);
            record.extend_from_slice(chunk);
            self.send(&record, deadline)?;
        }
        Ok(())
    }

    pub fn receive_message(
        &mut self,
        maximum: usize,
        deadline: Instant,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let result = self.receive_message_inner(maximum, deadline);
        if result.is_err() {
            self.cipher = None;
        }
        result
    }

    fn receive_message_inner(
        &mut self,
        maximum: usize,
        deadline: Instant,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let first = self.receive(deadline)?;
        self.assemble_message(first, maximum, deadline)
    }

    /// Wait for the next logical message on an idle channel. The first record
    /// is awaited with the transport's keepalive-bounded idle wait; once it has
    /// arrived, the remaining fragments must complete within `io_timeout`, so a
    /// peer that starts a message and stalls cannot hold the channel.
    pub fn receive_message_idle(
        &mut self,
        maximum: usize,
        io_timeout: Duration,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let result = self
            .receive_idle()
            .and_then(|first| self.assemble_message(first, maximum, Instant::now() + io_timeout));
        if result.is_err() {
            self.cipher = None;
        }
        result
    }

    fn assemble_message(
        &mut self,
        first: Zeroizing<Vec<u8>>,
        maximum: usize,
        deadline: Instant,
    ) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(first.len() >= 5 && first[0] == 1, "invalid message start");
        let total = u32::from_be_bytes(first[1..5].try_into()?) as usize;
        ensure!(total <= maximum, "message exceeds limit");
        ensure!(
            first.len() - 5 == total.min(MAX_PLAINTEXT_BYTES - 5),
            "invalid initial fragment size"
        );
        let mut message = Zeroizing::new(Vec::with_capacity(total));
        message.extend_from_slice(&first[5..]);
        while message.len() < total {
            let record = self.receive(deadline)?;
            let expected = (total - message.len()).min(MAX_PLAINTEXT_BYTES - 1);
            ensure!(
                record.len() == expected + 1 && record[0] == 2,
                "invalid continuation"
            );
            message.extend_from_slice(&record[1..]);
        }
        Ok(message)
    }
}
