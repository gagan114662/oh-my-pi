	#[pyclass]
	struct DeadlineHold {
		entered: Sender<()>,
		release: Receiver<()>,
	}

	#[pymethods]
	impl DeadlineHold {
		fn wait(&self, py: Python<'_>) -> PyResult<()> {
			let _ = self.entered.send(());
			// Release interpreter attachment while the native operation holds
			// completion. Bound even the failure path if the test unwinds.
			py.detach(|| self.release.recv_timeout(StdDuration::from_secs(6)))
				.map_err(|error| PyRuntimeError::new_err(error.to_string()))
		}
	}

	struct DeadlineInstaller {
		entered:    Sender<()>,
		release:    Receiver<()>,
		interrupts: Sender<()>,
		watchdog:   TimeoutHandle,
	}

	impl NamespaceInstaller for DeadlineInstaller {
		fn install(&self, py: Python<'_>, globals: &Bound<'_, PyDict>) -> PyResult<()> {
			globals.set_item(
				"hold",
				Py::new(py, DeadlineHold {
					entered: self.entered.clone(),
					release: self.release.clone(),
				})?,
			)
		}

		fn begin_cell(
			&self,
			_py: Python<'_>,
			_globals: &Bound<'_, PyDict>,
			_cell: &Bytes,
			timeout: Option<StdDuration>,
		) -> PyResult<TimeoutHandle> {
			assert_eq!(timeout, Some(StdDuration::from_secs(2)));
			Ok(self.watchdog.clone())
		}

		fn cancel_cell(&self, _cell: &Bytes) {
			let _ = self.interrupts.send(());
		}
	}

	#[tokio::test]
	async fn first_stop_reason_survives_later_watchdog_or_cancel_during_native_wait() {
		let _globals = PROCESS_GLOBALS.read();
		for cancel_first in [true, false] {
			let (entered_tx, entered_rx) = flume::unbounded();
			let (release_tx, release_rx) = flume::unbounded();
			let (interrupt_tx, interrupt_rx) = flume::unbounded();
			let watchdog = TimeoutHandle::new(Some(StdDuration::from_secs(2)));
			// Exclude worker initialization from the interleaving. Resume the
			// unchanged two-second window only after the native wait is entered.
			let setup_pause = watchdog.pause();
			let runtime = EmbeddedPython::with_installer(
				Arc::clone(&ENGINE),
				Arc::new(DeadlineInstaller {
					entered: entered_tx,
					release: release_rx,
					interrupts: interrupt_tx,
					watchdog,
				}),
				TEST_INTERRUPT_GRACE,
			)
			.expect("runtime");
			let session = runtime.open_session().await.expect("session");
			let mut run = runtime
				.run(&session, RunRequest {
					code:    sf!("hold.wait()"),
					timeout: Some(StdDuration::from_secs(2)),
					reset:   false,
					runtime: RuntimeSnapshot::default(),
				})
				.await
				.expect("held cell");
			assert!(matches!(run.next_event().await.expect("event"), Some(RunEvent::Started { .. })));
			time::timeout(StdDuration::from_secs(2), entered_rx.recv_async())
				.await
				.expect("native wait entered promptly")
				.expect("entry signal");
			if cancel_first {
				run.cancel().await.expect("explicit cancellation");
				interrupt_rx
					.recv_async()
					.await
					.expect("explicit cancellation reached host");
			}
			drop(setup_pause);
			time::timeout(StdDuration::from_secs(3), interrupt_rx.recv_async())
				.await
				.expect("actual two-second watchdog escalated")
				.expect("watchdog host signal");
			if !cancel_first {
				run.cancel().await.expect("late cancellation");
			}
			release_tx.send(()).expect("release native operation");
			let done = time::timeout(StdDuration::from_secs(2), completion(&mut run))
				.await
				.expect("held operation completes after release");
			assert_eq!(
				done.status.outcome,
				if cancel_first {
					CellOutcome::Cancelled
				} else {
					CellOutcome::Timeout
				}
			);
		}
	}

