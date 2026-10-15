#!/usr/bin/env python3
"""Run a lock wrapper with a bounded task deadline, including its lock queue."""
import datetime
import os
import signal
import subprocess


def run(command, env=None):
    timeout = None
    deadline = os.environ.get("PROFILE_STOP_UTC")
    if deadline:
        end = datetime.datetime.fromisoformat(deadline)
        timeout = (end - datetime.datetime.now(datetime.timezone.utc)).total_seconds()
        if timeout <= 0:
            raise TimeoutError("The task deadline has been reached before acquiring the host lock")
    process = subprocess.Popen(command, env=env, start_new_session=True)
    try:
        code = process.wait(timeout=timeout)
    except (subprocess.TimeoutExpired, KeyboardInterrupt):
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        raise
    if code:
        raise subprocess.CalledProcessError(code, command)
