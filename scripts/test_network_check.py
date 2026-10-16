#!/usr/bin/env python3
"""Offline regression tests: no subprocess or network probe is executed."""
import contextlib
import importlib.machinery
import importlib.util
import io
import json
from pathlib import Path
import stat
import tempfile
import threading
import unittest
from unittest.mock import patch

loader = importlib.machinery.SourceFileLoader(
    'network_check', str(Path(__file__).resolve().parents[1] / 'host/runtime/check-network'))
spec = importlib.util.spec_from_loader(loader.name, loader)
network = importlib.util.module_from_spec(spec)
loader.exec_module(network)

PING = '''64 bytes from 100.64.0.2: icmp_seq=1 ttl=64 time=1.000 ms
64 bytes from 100.64.0.2: icmp_seq=2 ttl=64 time=3.000 ms
64 bytes from 100.64.0.2: icmp_seq=2 ttl=64 time=8.000 ms (DUP!)
64 bytes from 100.64.0.2: icmp_seq=4 ttl=64 time=11.000 ms
4 packets transmitted, 3 received, +1 duplicates, 25% packet loss, time 3000ms
rtt min/avg/max/mdev = 1.000/5.750/11.000/4.023 ms
'''


class NetworkCheckTests(unittest.TestCase):
    def test_loss_percentile_and_duplicate_handling(self):
        result = network.icmp_stats(PING)
        self.assertEqual(result['packet_loss_percent'], 25)
        self.assertEqual(result['received'], 3)
        self.assertEqual(result['latency_samples_received'], 3)
        self.assertEqual(result['rtt_ms']['p95'], 11)
        self.assertEqual(result['rtt_ms']['avg'], 5.750)

    def test_no_replies_and_failed_probe_are_not_conflated(self):
        no_reply = network.icmp_stats('10 packets transmitted, 0 received, 100% packet loss, time 9000ms')
        self.assertEqual(no_reply['packet_loss_percent'], 100)
        self.assertIsNone(no_reply['rtt_ms']['p95'])
        self.assertIn('do not prove', no_reply['note'])
        failed = network.icmp_stats('ping: permission denied')
        self.assertIsNone(failed['packet_loss_percent'])
        self.assertIsNone(failed['received'])
        self.assertFalse(failed['summary_complete'])

    def test_partial_timeout_retains_replies_without_inventing_loss(self):
        result = network.icmp_stats('64 bytes from ::1: icmp_seq=1 ttl=64 time<1 ms')
        self.assertEqual(result['latency_samples_received'], 1)
        self.assertEqual(result['upper_bound_samples'], 1)
        self.assertIsNone(result['packet_loss_percent'])

    def test_direct_observation_is_separate_from_relay_and_icmp(self):
        output = ('pong from phone (100.64.0.2) via DERP(test) in 230ms\n'
                  'pong from phone (100.64.0.2) via peer-relay(test) in 30ms\n'
                  'pong from phone (100.64.0.2) via 192.168.1.20:41641 in 6ms\n')
        result = network.path_observations(output)
        self.assertTrue(result['direct_observed'])
        self.assertEqual([x['kind'] for x in result['observations']], ['derp', 'peer-relay', 'direct'])
        self.assertFalse(network.path_observations(output.splitlines()[0])['direct_observed'])
        self.assertFalse(network.path_observations(PING)['direct_observed'])
        self.assertTrue(network.path_observations(
            'pong from phone (100.64.0.2) via [fd00::2]:41641 in 6ms')['direct_observed'])

    def test_cli_limits_and_unicast_only(self):
        self.assertEqual(network.parser().parse_args(['phone']).samples, 30)
        for argv in (['phone', '--samples', '9'], ['phone', '--samples', '121'],
                     ['phone', '--lan-peer', '224.0.0.1'], ['255.255.255.255'],
                     ['phone', '--lan-peer', '::'], ['-bad']):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                network.parser().parse_args(argv)
        self.assertEqual(network.parser().parse_args(['phone', '--lan-peer', 'fe80::1%wlan0']).lan_peer,
                         'fe80::1%wlan0')

    def test_ipv6_gateway_gets_interface_scope(self):
        result = network.default_gateway({
            'routes': {'exit_code': 0, 'output': '[]'},
            'routes6': {'exit_code': 0, 'output': json.dumps([
                {'dst': 'default', 'gateway': 'fe80::1', 'dev': 'wlan0', 'metric': 600}])}})
        self.assertEqual(result, 'fe80::1%wlan0')

    def test_peer_metadata_filters_unrelated_devices(self):
        fixture = {'Peer': {'one': {'HostName': 'phone', 'TailscaleIPs': ['100.64.0.2'],
                                   'Online': False, 'LastSeen': 'test-time', 'Relay': 'region'},
                            'two': {'HostName': 'private-other-device', 'Online': True}}}
        with patch.object(network, 'run_command', return_value={'exit_code': 0, 'output': json.dumps(fixture)}):
            result = network.selected_peer_status('100.64.0.2')
        self.assertTrue(result['available'])
        self.assertFalse(result['Online'])
        self.assertNotIn('private-other-device', json.dumps(result))
        self.assertNotIn('HostName', result)

    def test_guest_probe_and_p99(self):
        def run(args,timeout):
            if args[0]=='ip':return {'exit_code':0,'output':'[]'}
            return {'exit_code':0,'output':PING}
        with patch.object(network,'run_command',side_effect=run):
            report=network.collect('100.64.0.2',10,guest_peer='172.30.61.12')
        probe=report['tests']['guest_icmp']
        self.assertEqual(probe['statistics']['received'],3)
        self.assertEqual(probe['statistics']['rtt_ms']['p99'],11)
        self.assertEqual(report['guest_peer'],'172.30.61.12')

    def test_three_icmp_targets_start_concurrently_and_report_is_private(self):
        barrier = threading.Barrier(3, timeout=3)
        commands = []

        def fake_command(args, timeout):
            commands.append((args, timeout))
            if args[0] == 'ip':
                output = '[]' if '-6' in args else '[{"dst":"default","gateway":"192.168.1.1"}]'
            elif args[0] == 'ping':
                barrier.wait()
                output = PING
            elif args[:2] == ['tailscale', 'ping']:
                output = 'pong from phone (100.64.0.2) via 192.168.1.20:41641 in 2ms\n'
            else:
                output = 'UDP: true'
            return {'exit_code': 0, 'output': output}

        with tempfile.TemporaryDirectory() as folder:
            with patch.object(network, 'run_command', side_effect=fake_command), \
                 patch.object(network, 'REPORT_DIR', Path(folder)), \
                 patch.object(network.subprocess, 'run') as forbidden, \
                 contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(network.main(['100.64.0.2', '--lan-peer', '192.168.1.20', '--samples', '10']), 0)
                forbidden.assert_not_called()
            summary = json.loads(output.getvalue())
            report = json.loads(Path(summary['report']).read_text())
            self.assertEqual(stat.S_IMODE(Path(summary['report']).stat().st_mode), 0o600)
        self.assertTrue(report['direct_verified'])
        self.assertEqual(report['direct_verified'], report['direct_observed'])
        self.assertIn('NOT a stability verdict', report['direct_verified_meaning'])
        self.assertEqual(set(summary['icmp']), {'icmp', 'gateway_icmp', 'lan_icmp'})
        for args, timeout in commands:
            if args[0] == 'ping':
                self.assertEqual(args[args.index('-i') + 1], '1')
                self.assertNotIn('-b', args)
                self.assertLessEqual(timeout, 20)
            if args[:2] == ['tailscale', 'ping']:
                self.assertIn('--until-direct=false', args)


if __name__ == '__main__':
    unittest.main()
