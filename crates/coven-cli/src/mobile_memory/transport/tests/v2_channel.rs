use super::super::routing::{
    self,
    channel::{self, DeviceAdmission, HostAdmission},
};
use super::*;

fn exchange(fail_device: bool, fail_host: bool) -> (usize, usize, Vec<&'static str>, usize) {
    let (private, public) = generate_noise_key().unwrap();
    let pairing = pairing(public);
    let host = identity(1);
    let device = identity(2);
    let binding = pairing.sign_noise_binding(&host).unwrap();
    let (client, server, capture) = pipes();
    let trace = Arc::new(Mutex::new(Vec::new()));
    let host_trace = trace.clone();
    let host_capture = capture.clone();
    let server_pairing = pairing.clone();
    let host_thread = thread::spawn(move || {
        let admission = HostAdmission {
            pairing: &server_pairing,
            private: &private,
            host: &host,
            context: &[7; 32],
            direct: false,
            proof: routing::HostState {
                generation: 5,
                floor: 8,
            }
            .proof(9)
            .unwrap(),
        };
        let result = channel::accept(
            server,
            admission,
            |_, _| Ok(()),
            |proof, _deadline| Ok(proof),
            |proof| {
                assert_eq!(proof.counter, 9);
                // Two Noise messages, two nonces, host proof, device proof.
                assert_eq!(host_capture.lock().unwrap().len(), 6);
                host_trace.lock().unwrap().push("host-commit");
                if fail_host {
                    bail!("synthetic host storage failure");
                }
                Ok(())
            },
        );
        if result.is_ok() {
            1
        } else {
            0
        }
    });
    let admission = DeviceAdmission {
        pairing: &pairing,
        binding: &binding,
        device: &device,
        context: &[7; 32],
        direct: false,
        state: routing::DeviceState {
            generation: 5,
            next: 9,
            floor: 8,
        },
    };
    let result = channel::connect(client, admission, |state| {
        assert_eq!(
            state,
            routing::DeviceState {
                generation: 5,
                next: 10,
                floor: 8
            }
        );
        assert_eq!(capture.lock().unwrap().len(), 5);
        trace.lock().unwrap().push("device-persist");
        if fail_device {
            bail!("synthetic device storage failure");
        }
        Ok(())
    });
    let client_ok = usize::from(result.is_ok());
    let host_ok = host_thread.join().unwrap();
    let events = trace.lock().unwrap().clone();
    let frames = capture.lock().unwrap().len();
    (client_ok, host_ok, events, frames)
}

#[test]
fn v2_counter_persistence_precedes_device_proof_and_host_completion() {
    let (client, host, events, frames) = exchange(false, false);
    assert_eq!((client, host), (1, 1));
    assert_eq!(events, vec!["device-persist", "host-commit"]);
    assert_eq!(frames, 8);
}
#[test]
fn v2_failed_device_persistence_sends_no_device_proof() {
    let (client, host, events, frames) = exchange(true, false);
    assert_eq!((client, host), (0, 0));
    assert_eq!(events, vec!["device-persist"]);
    assert_eq!(frames, 5);
}
#[test]
fn v2_failed_host_persistence_sends_no_completion() {
    let (client, host, events, frames) = exchange(false, true);
    assert_eq!((client, host), (0, 0));
    assert_eq!(events, vec!["device-persist", "host-commit"]);
    assert_eq!(frames, 6);
}
