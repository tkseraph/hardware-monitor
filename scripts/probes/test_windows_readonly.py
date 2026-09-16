"""Synthetic tests of the production probe's association/aggregation boundary."""
import unittest
from windows_readonly import adapter_key, busiest_engine


def instance(pid=1, engine=0, adapter=2):
    return f'pid_{pid}_luid_0x00000000_0x{adapter:08x}_phys_0_eng_{engine}_engtype_3D'


class WindowsProbeTests(unittest.TestCase):
    def test_association_parses_private_address_without_returning_name(self):
        self.assertEqual(adapter_key(instance()), (0, 2, 0))
        self.assertIsNone(adapter_key('unrelated'))

    def test_aggregates_processes_per_engine_then_uses_busiest(self):
        rows = [(instance(1), 20), (instance(2), 30), (instance(3, 1), 40),
                (instance(4, adapter=3), 99)]
        self.assertEqual(busiest_engine(rows, (0, 2, 0)), 50)

    def test_missing_differs_from_measured_zero(self):
        self.assertIsNone(busiest_engine([], (0, 2, 0)))
        self.assertEqual(busiest_engine([(instance(), 0)], (0, 2, 0)), 0)

    def test_invalid_or_duplicate_values_are_not_clamped(self):
        for rows in [[(instance(), 101)], [(instance(), -1)],
                     [(instance(), float('nan'))], [(instance(), float('inf'))],
                     [(instance(), 1), (instance(), 2)],
                     [(instance(1), 60), (instance(2), 60)]]:
            self.assertIsNone(busiest_engine(rows, (0, 2, 0)))

    def test_unknown_engine_is_not_silently_ignored(self):
        self.assertIsNone(busiest_engine([('luid_0x0_0x2_phys_0', 4)], (0, 2, 0)))


if __name__ == '__main__':
    unittest.main()
