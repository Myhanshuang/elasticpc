#!/usr/bin/env python3
"""Offline acceptance-runner regressions; commands and Guest state are simulated.

Only private ordinary files are created. No service, QEMU, network or real disk
is contacted. Human observations stay distinct from process readiness.
"""
import contextlib
from collections import deque
import fcntl
import hashlib
import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
loader = importlib.machinery.SourceFileLoader('epc_guided_test', str(ROOT / 'host/runtime/epc-test'))
spec = importlib.util.spec_from_loader(loader.name, loader)
guided = importlib.util.module_from_spec(spec)
loader.exec_module(guided)
PRODUCTION_GUESTS = guided.GUESTS
SECRET = 'TEST-CONSOLE-SECRET-never-save'


class FakeOps:
    """Models asynchronous start/stop without bypassing the real runner logic."""
    def __init__(self, case, answers=()):
        self.case = case
        self.answers = deque(answers)
        self.commands = []
        self.waits = []
        self.elapsed = 0
        self.services = {'windows': 'inactive', 'manjaro': 'inactive'}
        self.on_command = None
        self.generation = 0

    def clock(self):
        return self.elapsed

    def wait(self, seconds):
        self.waits.append(seconds)
        self.elapsed += seconds

    def ask(self, prompt):
        if not self.answers:
            raise EOFError('Simulated terminal EOF')
        answer = self.answers.popleft()
        if isinstance(answer, BaseException):
            raise answer
        return answer

    def command(self, args, timeout=60, live=False):
        args = [str(value) for value in args]
        self.commands.append((args, timeout, live))
        if self.on_command:
            result = self.on_command(args, timeout, live)
            if result is not None:
                return result
        output = ''
        if args[:2] == ['systemctl', 'show']:
            output = self.services[args[2].removeprefix('elasticpc-')] + '\n'
        elif args == ['tailscale', 'ip', '-4']:
            output = '100.64.1.2\n'
        elif args[0] == guided.EPC:
            command, name = args[1:3]
            if command == 'start':
                self.generation += 1
                self.case.make_state(name, f'{self.generation:012d}', status='running', archive='pending', mode=args[args.index('--mode') + 1])
                self.services[name] = 'active'
            elif command == 'stop':
                state = self.case.read_state(name)
                self.case.make_state(name, Path(state['session']).name.removeprefix(name + '-' + state['mode'] + '-'), mode=state['mode'])
                if '--discard' in args:
                    state=self.case.read_state(name)
                    state.update(status='discarded',archive='discarded',discard_requested=True)
                    Path(state['session'],'archive.json').write_text(json.dumps({'status':'discarded','requested_by_user':True}))
                    (self.case.base/(name+'-latest.json')).write_text(json.dumps(state))
                self.services[name] = 'inactive'
            elif command == 'status':
                state = self.case.read_state(name)
                state['qmp'] = {'status': 'running'} if self.services[name] == 'active' else 'unreachable'
                output = json.dumps(state)
            elif command in ('console', 'password'):
                output = SECRET
                if live:
                    print(SECRET)
        return {'exit_code': 0, 'output': output}


class GuidedTestRegressions(unittest.TestCase):
    def setUp(self):
        order=patch.object(guided,'GUESTS',('windows','manjaro'));order.start();self.addCleanup(order.stop)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.base = self.root / 'runtime'
        self.run_dir = self.root / 'run'
        self.report_dir = self.root / 'report'
        for folder in (self.base, self.run_dir, self.report_dir):
            folder.mkdir()
        for name, value in (('BASE', self.base), ('RUN', self.run_dir)):
            patcher = patch.object(guided, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        forbidden = patch.object(guided.subprocess, 'run', side_effect=AssertionError('Unexpected real command'))
        forbidden.start()
        self.addCleanup(forbidden.stop)
        self.ops = FakeOps(self)
        self.qmp_online = False
        self.test = guided.TestRun(self.ops, {'qmp': self.qmp}, self.report_dir)

    def qmp(self, sock, command):
        if self.qmp_online:
            return {'status': 'running'}
        raise ConnectionRefusedError('No synthetic QEMU')

    def read_state(self, name='windows'):
        path = self.base / (name + '-latest.json')
        return json.loads(path.read_text()) if path.exists() else {}

    def make_state(self, name='windows', identity='000000000000', status='stopped', archive='saved', mode='trial'):
        session = self.base / f'{name}-{mode}-{identity}'
        session.mkdir(exist_ok=True)
        (session / 'store').mkdir(exist_ok=True)
        state = {'os': name, 'mode': mode, 'storage': 'ram' if mode == 'trial' else 'native', 'status': status, 'exit_code': 0,
                 'archive': archive, 'session': str(session), 'started_at': time.time()}
        if mode == 'direct':
            state.pop('archive', None)
        if archive == 'saved' and mode == 'trial':
            data = b'synthetic saved COW data'
            saved = session / 'trial-saved.qcow2'
            saved.write_bytes(data)
            (session / 'archive.json').write_text(json.dumps({
                'status': 'saved', 'path': str(saved), 'bytes': len(data),
                'sha256': hashlib.sha256(data).hexdigest(), 'time': time.time()}))
        (session / 'state.json').write_text(json.dumps(state))
        (self.base / (name + '-latest.json')).write_text(json.dumps(state))
        return state

    def execute_flow(self, answers):
        self.ops.answers = deque(answers)
        with contextlib.redirect_stdout(io.StringIO()):
            try:
                self.test.run()
            except (Exception, KeyboardInterrupt) as error:
                self.test.interrupted(str(error) or 'Interrupted')
        return json.loads((self.report_dir / 'summary.json').read_text())

    def control_commands(self):
        return [args for args, _, _ in self.ops.commands if args and args[0] == guided.EPC]

    def test_saved_guest_requires_independent_quiescence_checks(self):
        self.make_state()
        self.assertTrue(self.test.gate('windows', require_archive=True)['clear'])
        for service in ('active', 'activating', 'deactivating', 'unknown'):
            with self.subTest(service=service):
                self.ops.services['windows'] = service
                self.assertFalse(self.test.gate('windows', require_archive=True)['clear'])
        self.ops.services['windows'] = 'inactive'
        self.qmp_online = True
        self.assertFalse(self.test.gate('windows', require_archive=True)['clear'])
        self.qmp_online = False
        with patch.object(Path, 'is_mount', return_value=True):
            self.assertFalse(self.test.gate('windows', require_archive=True)['clear'])

    def test_busy_lock_blocks_even_when_saved_and_service_inactive(self):
        self.make_state()
        with (self.run_dir / 'windows.lock').open('w') as held:
            fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result = self.test.gate('windows', require_archive=True)
        self.assertFalse(result['clear'])
        self.assertTrue(result['reasons'])

    def test_missing_or_inconsistent_archive_cannot_pass(self):
        state = self.make_state()
        session = Path(state['session'])
        archive = session / 'archive.json'
        saved = session / 'trial-saved.qcow2'
        for change in ('missing-file', 'missing-metadata', 'wrong-length', 'wrong-path'):
            with self.subTest(change=change):
                self.make_state()
                if change == 'missing-file':
                    saved.unlink()
                elif change == 'missing-metadata':
                    archive.unlink()
                else:
                    record = json.loads(archive.read_text())
                    record['bytes' if change == 'wrong-length' else 'path'] = -1 if change == 'wrong-length' else '/somewhere-else'
                    archive.write_text(json.dumps(record))
                self.assertFalse(self.test.gate('windows', require_archive=True)['clear'])

    def test_cleanup_failure_blocks_following_guest(self):
        self.make_state(status='cleanup-failed')
        with self.assertRaises(guided.Halt):
            self.test.all_clear()

    def test_live_matching_qemu_process_blocks_even_with_unreachable_qmp(self):
        state = self.make_state()
        state['pid'] = 424242
        (self.base / 'windows-latest.json').write_text(json.dumps(state))
        original = Path.read_bytes

        def process_cmdline(path):
            if path == Path('/proc/424242/cmdline'):
                return b'qemu-system-x86_64\0-qmp\0unix:' + str(Path(state['session']) / 'qmp.sock').encode() + b'\0'
            return original(path)

        with patch.object(Path, 'read_bytes', process_cmdline):
            self.assertFalse(self.test.gate('windows', require_archive=True)['clear'])

    def test_session_identity_change_blocks_control(self):
        state = self.make_state(identity='old')
        self.make_state(identity='replacement')
        with self.assertRaises(guided.Halt):
            self.test.still_ours('windows', state['session'])
        self.assertFalse(self.control_commands())

    def test_trial_path_must_not_point_to_native_or_unrelated_session(self):
        for path in ('/tmp/foreign', str(self.base / 'windows-direct-any'), str(self.base / 'manjaro-trial-other')):
            with self.subTest(path=path):
                (self.base / 'windows-latest.json').write_text(json.dumps({'session': path}))
                with self.assertRaises(guided.Halt):
                    self.test.state('windows')

    def test_failed_preflight_never_reuses_old_saved_session(self):
        old = self.make_state(identity='old')

        def reject_start(args, timeout, live):
            if args[:3] == [guided.EPC, 'start', 'windows']:
                (self.base / 'windows-last-error.json').write_text(json.dumps({
                    'status': 'failed', 'error': 'source mounted', 'time': time.time()}))
                return {'exit_code': 0, 'output': 'Startup submitted'}
            if args[:3] == [guided.EPC, 'status', 'windows']:
                return {'exit_code': 0, 'output': json.dumps({'status': 'failed', 'error': 'source mounted'})}
            return None

        self.ops.on_command = reject_start
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(guided.Halt):
            self.test.start('windows', False)
        stage = self.test.summary['stages'][0]
        self.assertFalse(stage['automated_qmp_ready'])
        self.assertNotEqual(stage.get('session'), old['session'])
        self.assertFalse(stage['archive_gate_passed'])

    def test_fresh_error_cannot_be_overridden_by_running_state(self):
        def new_error(args, timeout, live):
            if args[:3] == [guided.EPC, 'start', 'windows']:
                self.make_state(identity='new', status='running', archive='pending')
                (self.base / 'windows-last-error.json').write_text(json.dumps({
                    'status': 'failed', 'error': 'new startup error', 'time': time.time() + 1}))
                self.ops.services['windows'] = 'active'
                return {'exit_code': 0, 'output': 'Startup submitted'}
            return None

        self.ops.on_command = new_error
        with contextlib.redirect_stdout(io.StringIO()):
            try:
                stage = self.test.start('windows', False)
            except guided.Halt:
                stage = self.test.summary['stages'][0]
        self.assertFalse(stage['automated_qmp_ready'])

    def test_start_timeout_is_bounded_and_respects_150_second_poll_interval(self):
        def no_session_yet(args, timeout, live):
            if args[:3] == [guided.EPC, 'start', 'windows']:
                self.ops.services['windows'] = 'activating'
                return {'exit_code': 0, 'output': 'Starting'}
            return None

        self.ops.on_command = no_session_yet
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(guided.Halt):
            self.test.start('windows', False)
        self.assertTrue(self.ops.waits)
        self.assertTrue(all(interval >= 150 for interval in self.ops.waits))
        self.assertLessEqual(self.ops.elapsed, 600)
        self.assertFalse(any('--force' in args for args in self.control_commands()))

    def test_eof_or_ctrl_c_retains_partial_report_without_force_or_host_shutdown(self):
        for interruption in (EOFError('terminal closed'), KeyboardInterrupt()):
            with self.subTest(interruption=type(interruption).__name__):
                self.test = guided.TestRun(self.ops, {'qmp': self.qmp}, self.report_dir)
                summary = self.execute_flow(['', 'S', '', interruption])
                self.assertEqual(summary['status'], 'incomplete')
                self.assertFalse(summary['safe_to_poweroff'])
                self.assertFalse(summary['stages'][0]['archive_gate_passed'])
                self.assertEqual(summary['stages'][0]['human_result'], 'not_observed')
                for args, _, _ in self.ops.commands:
                    self.assertNotIn('--force', args)
                    self.assertNotIn(args[0], ('poweroff', 'shutdown', 'reboot'))
                # Reset synthetic process state before the next independent run.
                self.ops.services['windows'] = 'inactive'
                self.make_state()

    def test_direct_shutdown_requires_clean_exit_without_archive(self):
        state = self.make_state('manjaro')
        state = self.read_state('manjaro')
        session = self.base / 'manjaro-direct-fixture'
        session.mkdir()
        state.update(session=str(session), mode='direct', status='stopped', exit_code=0)
        state.pop('archive', None)
        (self.base / 'manjaro-latest.json').write_text(json.dumps(state))
        runner = self.test
        self.assertTrue(runner.gate('manjaro', require_archive=True)['clear'])
        state['status'] = 'forced-stopped'
        (self.base / 'manjaro-latest.json').write_text(json.dumps(state))
        self.assertFalse(runner.gate('manjaro', require_archive=True)['clear'])

    def test_completed_flow_uses_mixed_modes_and_keeps_credentials_out_of_reports(self):
        summary = self.execute_flow(['', 'S', '', 'P', '15 seconds', '', '', 'P', '12 seconds', ''])
        self.assertEqual(summary['status'], 'completed')
        self.assertTrue(summary['safe_to_poweroff'])
        self.assertEqual([stage['guest'] for stage in summary['stages']], ['windows', 'manjaro'])
        for args in self.control_commands():
            self.assertNotIn('accept-trial', args)
            self.assertNotIn('--force', args)
            if args[1] == 'start':
                self.assertEqual(args[3:], ['--mode', 'direct'] if args[2] == 'manjaro' else ['--mode','trial','--gpu'])
        for args, _, live in self.ops.commands:
            self.assertNotIn(args[0], ('poweroff', 'shutdown', 'reboot'))
            if args[0] == guided.EPC and args[1] in ('console', 'password'):
                self.assertTrue(live)
        saved = '\n'.join(path.read_text() for path in self.report_dir.rglob('*.json'))
        self.assertNotIn(SECRET, saved)

    def test_production_order_is_linux_then_windows_gpu(self):
        self.assertEqual(PRODUCTION_GUESTS,('manjaro','windows'))
        with patch.object(guided,'GUESTS',PRODUCTION_GUESTS):
            summary=self.execute_flow(['','S','','P','','','P','','DISCARD'])
        self.assertTrue(summary['safe_to_poweroff'])
        self.assertEqual([s['guest'] for s in summary['stages']],['manjaro','windows'])
        starts=[a for a in self.control_commands() if a[1]=='start']
        self.assertEqual(starts,[[guided.EPC,'start','manjaro','--mode','direct'],[guided.EPC,'start','windows','--mode','trial','--gpu']])

    def test_explicit_discard_allows_cleanup_but_not_success(self):
        summary=self.execute_flow(['','S','','P','','DISCARD','','P',''])
        self.assertTrue(summary['safe_to_poweroff'])
        self.assertEqual(summary['status'],'completed_with_failures')
        self.assertTrue(summary['stages'][0]['discarded'])
        self.assertIn([guided.EPC,'stop','windows','--force','--discard'],self.control_commands())

    def test_failed_desktop_keeps_failure_with_requested_gpu(self):
        summary = self.execute_flow(['', 'S', '', 'F', 'HYPERVISOR_ERROR', '', '', 'P', 'GUI works'])
        self.assertEqual(summary['status'], 'completed_with_failures')
        self.assertEqual(summary['stages'][0]['human_result'], 'failed')
        self.assertTrue(summary['stages'][0]['gpu'])
        self.assertTrue(summary['safe_to_poweroff'])
        self.assertIn('HYPERVISOR_ERROR', json.dumps(summary))

    def test_console_failure_is_not_hidden_by_human_desktop_pass(self):
        def broken_console(args, timeout, live):
            if args[:2] == [guided.EPC, 'console']:
                return {'exit_code': 1, 'output': 'Console unavailable'}
            return None

        self.ops.on_command = broken_console
        summary = self.execute_flow(['', 'S', '', 'P', '', '', '', 'P', '', ''])
        self.assertEqual(summary['status'], 'completed_with_failures')
        self.assertFalse(summary['stages'][0]['console_commands_ok'])
        self.assertFalse(summary['stages'][1]['console_commands_ok'])

    def test_archive_failure_does_not_launch_next_guest_or_claim_safe_shutdown(self):
        def failed_archive(args, timeout, live):
            if args[:3] == [guided.EPC, 'stop', 'windows']:
                state = self.read_state()
                identity = Path(state['session']).name.removeprefix('windows-trial-')
                self.make_state(identity=identity, status='save-failed', archive='failed')
                self.ops.services['windows'] = 'inactive'
                return {'exit_code': 0, 'output': 'USB full; RAM retained'}
            return None

        self.ops.on_command = failed_archive
        summary = self.execute_flow(['', 'S', '', 'P', '', ''])
        self.assertEqual(summary['status'], 'incomplete')
        self.assertFalse(summary['safe_to_poweroff'])
        self.assertFalse(summary['stages'][0]['archive_gate_passed'])
        starts = [args for args in self.control_commands() if args[1] == 'start']
        self.assertEqual(len(starts), 1)
        self.assertEqual(starts[0][2], 'windows')

    def test_reset_requires_explicit_reset_choice(self):
        state = self.make_state(status='running', archive='pending')
        stage = {'guest': 'windows', 'gpu': False, 'session': state['session'],
                 'automated_qmp_ready': True, 'archive_gate_passed': False}
        self.test.summary['stages'].append(stage)
        self.ops.answers = deque(['R', '', 'P', ''])
        with contextlib.redirect_stdout(io.StringIO()):
            self.test.observe('windows', stage)
        self.assertFalse(any('--force' in args for args in self.control_commands()))

    def test_unprivileged_entry_refuses_before_loading_runtime(self):
        with patch.object(guided.os, 'geteuid', return_value=1000), \
             patch.object(guided.runpy, 'run_path') as runtime:
            with self.assertRaises(guided.Halt):
                guided.main([])
            runtime.assert_not_called()


if __name__ == '__main__':
    unittest.main()
