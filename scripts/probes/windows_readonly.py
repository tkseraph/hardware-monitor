"""Opt-in Windows source probe. Stdlib only; no elevation, ETW or disk writes.

Reports an allowlist of aggregate numbers. Raw PDH instance names (which may
contain PIDs/volume paths), LUIDs and exception messages never leave the probe.
This does not start the app, mutate settings or write monitor history.
"""
import argparse
import ctypes as c
import json
import math
import os
import re
import time
import uuid
from collections import defaultdict

DWORD = c.c_uint32
HANDLE = c.c_void_p
MORE_DATA = 0x800007D2
VALID_DATA = {0, 1}
MAX_BUFFER = 4 * 1024 * 1024
FORMAT = 0x200 | 0x1000 | 0x8000  # DOUBLE | NOSCALE | NOCAP100
LUID_RE = re.compile(r'luid_0x([0-9a-f]+)_0x([0-9a-f]+)_phys_(\d+)', re.I)
ENGINE_RE = re.compile(r'_eng_(\d+)(?:_|$)', re.I)


def adapter_key(name):
    match = LUID_RE.search(name)
    return tuple(int(part, 16 if i < 2 else 10)
                 for i, part in enumerate(match.groups())) if match else None


def busiest_engine(rows, key):
    """Per-process engine counters -> per-engine sum, then busiest engine.

    Reject ambiguous/duplicate/out-of-range input; never clamp bad aggregation.
    The result is a candidate statistic, not proof of Task Manager equivalence.
    """
    engines = defaultdict(float)
    seen = set()
    for name, value in rows:
        if adapter_key(name) != key:
            continue
        engine = ENGINE_RE.search(name)
        if engine is None or name in seen or not math.isfinite(value) or value < 0:
            return None
        seen.add(name)
        engines[int(engine.group(1))] += value
    if not engines or max(engines.values()) > 100:
        return None
    return max(engines.values())


class Luid(c.Structure):
    _fields_ = [('low', DWORD), ('high', c.c_int32)]


class AdapterDesc(c.Structure):
    _fields_ = [('description', c.c_wchar * 128), ('vendor', DWORD),
                ('device', DWORD), ('subsys', DWORD), ('revision', DWORD),
                ('dedicated_video', c.c_size_t), ('dedicated_system', c.c_size_t),
                ('shared_system', c.c_size_t), ('luid', Luid), ('flags', DWORD)]


def com_call(ptr, index, restype, argtypes, *args):
    table = c.cast(ptr, c.POINTER(c.POINTER(c.c_void_p))).contents
    return c.WINFUNCTYPE(restype, c.c_void_p, *argtypes)(table[index])(ptr, *args)


def dxgi_adapters():
    dxgi = c.WinDLL('dxgi.dll')
    dxgi.CreateDXGIFactory1.argtypes = [c.c_void_p, c.POINTER(HANDLE)]
    dxgi.CreateDXGIFactory1.restype = c.c_int32
    iid = (c.c_ubyte * 16).from_buffer_copy(uuid.UUID('770aae78-f26f-4dba-a829-253c83d1b387').bytes_le)
    factory = HANDLE()
    if dxgi.CreateDXGIFactory1(c.byref(iid), c.byref(factory)) < 0:
        return [], 'error'
    adapters = []
    try:
        for index in range(32):
            adapter = HANDLE()
            hr = com_call(factory, 12, c.c_int32, [DWORD, c.POINTER(HANDLE)], index, c.byref(adapter))
            if hr & 0xFFFFFFFF == 0x887A0002:
                return adapters, 'ok'
            if hr < 0:
                return adapters, 'error'
            try:
                desc = AdapterDesc()
                hr = com_call(adapter, 10, c.c_int32, [c.POINTER(AdapterDesc)], c.byref(desc))
                if hr >= 0:
                    adapters.append({
                        '_key': (desc.luid.high & 0xFFFFFFFF, desc.luid.low, 0),
                        'alias': f'adapter-{index + 1}',
                        'vendor_id': desc.vendor,
                        'software': bool(desc.flags & 2),
                        'remote': bool(desc.flags & 1),
                        'dedicated_video_capacity_bytes': desc.dedicated_video,
                        'shared_system_capacity_bytes': desc.shared_system,
                    })
            finally:
                com_call(adapter, 2, DWORD, [])
        return adapters, 'enumeration_limit'
    finally:
        com_call(factory, 2, DWORD, [])


class ValueUnion(c.Union):
    _fields_ = [('double', c.c_double), ('integer', c.c_int64)]


class FmtValue(c.Structure):
    _fields_ = [('status', DWORD), ('value', ValueUnion)]


class FmtItem(c.Structure):
    _fields_ = [('name', c.c_wchar_p), ('value', FmtValue)]


class PdhQuery:
    def __init__(self):
        self.dll = c.WinDLL('pdh.dll')
        signatures = {
            'PdhOpenQueryW': [c.c_wchar_p, c.c_size_t, c.POINTER(HANDLE)],
            'PdhAddEnglishCounterW': [HANDLE, c.c_wchar_p, c.c_size_t, c.POINTER(HANDLE)],
            'PdhCollectQueryData': [HANDLE],
            'PdhGetFormattedCounterArrayW': [HANDLE, DWORD, c.POINTER(DWORD), c.POINTER(DWORD), c.c_void_p],
            'PdhCloseQuery': [HANDLE],
        }
        for name, args in signatures.items():
            fn = getattr(self.dll, name)
            fn.argtypes = args
            fn.restype = DWORD
        self.handle = HANDLE()
        code = self.dll.PdhOpenQueryW(None, 0, c.byref(self.handle))
        if code:
            raise RuntimeError('pdh_open_failed')
        self.counters = {}

    def add(self, key, path):
        counter = HANDLE()
        code = self.dll.PdhAddEnglishCounterW(self.handle, path, 0, c.byref(counter))
        self.counters[key] = (counter, code)

    def collect(self):
        return self.dll.PdhCollectQueryData(self.handle)

    def read(self, key):
        counter, code = self.counters[key]
        if code:
            return [], f'add_error_0x{code:08x}'
        for _ in range(3):
            size, count = DWORD(), DWORD()
            code = self.dll.PdhGetFormattedCounterArrayW(counter, FORMAT, c.byref(size), c.byref(count), None)
            if code != MORE_DATA:
                return [], f'no_valid_data_0x{code:08x}'
            if size.value > MAX_BUFFER or not size.value:
                return [], 'buffer_limit'
            capacity = size.value
            buffer = c.create_string_buffer(capacity)
            code = self.dll.PdhGetFormattedCounterArrayW(counter, FORMAT, c.byref(size), c.byref(count), buffer)
            if code == MORE_DATA:
                continue
            if code:
                return [], f'read_error_0x{code:08x}'
            if count.value * c.sizeof(FmtItem) > capacity:
                return [], 'invalid_count'
            items = c.cast(buffer, c.POINTER(FmtItem))
            rows = []
            for i in range(count.value):
                item = items[i]
                value = item.value.value.double
                if item.value.status in VALID_DATA and math.isfinite(value) and value >= 0:
                    rows.append((item.name or '', value))
            return rows, ('ok' if rows else 'no_instances') if len(rows) == count.value else 'partial'
        return [], 'instance_churn'

    def close(self):
        self.dll.PdhCloseQuery(self.handle)


def run(samples, interval):
    adapters, dxgi_status = dxgi_adapters()
    query = PdhQuery()
    paths = {
        'gpu_engine': r'\GPU Engine(*)\Utilization Percentage',
        'gpu_dedicated': r'\GPU Adapter Memory(*)\Dedicated Usage',
        'gpu_shared': r'\GPU Adapter Memory(*)\Shared Usage',
        'disk_read': r'\PhysicalDisk(*)\Disk Read Bytes/sec',
        'disk_write': r'\PhysicalDisk(*)\Disk Write Bytes/sec',
    }
    report = {'schema_version': 1, 'mode': 'read_only_no_elevation',
              'started_at_unix': int(time.time()),
              'dxgi_status': dxgi_status, 'samples': [],
              'process_storage_io': {'status': 'unverified',
                  'reason': 'GetProcessIoCounters covers all IO; ETW not enabled'}}
    start = time.monotonic()
    try:
        for key, path in paths.items():
            query.add(key, path)
        query.collect()
        for _ in range(samples):
            time.sleep(interval)
            collection_code = query.collect()
            data = {key: query.read(key) for key in paths}
            snapshot = {'elapsed_secs': round(time.monotonic() - start, 3),
                        'collection_code': collection_code,
                        'sources': {key: {'status': state, 'valid_instances': len(rows)}
                                    for key, (rows, state) in data.items()}, 'adapters': []}
            for adapter in adapters:
                item = {key: value for key, value in adapter.items() if not key.startswith('_')}
                key = adapter['_key']
                item['candidate_busiest_engine_percent'] = (
                    busiest_engine(data['gpu_engine'][0], key)
                    if data['gpu_engine'][1] == 'ok' and collection_code == 0 else None)
                for metric in ('gpu_dedicated', 'gpu_shared'):
                    values = [v for name, v in data[metric][0] if adapter_key(name) == key]
                    item[metric + '_usage_bytes'] = int(values[0]) if len(values) == 1 else None
                snapshot['adapters'].append(item)
            for metric in ('disk_read', 'disk_write'):
                values = [v for name, v in data[metric][0] if re.match(r'^\d+(?: |$)', name)]
                snapshot['sources'][metric]['physical_instances'] = len(values)
                snapshot['sources'][metric]['total_bytes_per_sec'] = sum(values) if values else None
            report['samples'].append(snapshot)
    finally:
        query.close()
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--samples', type=int, default=3, choices=range(1, 11))
    parser.add_argument('--interval', type=float, default=1.0)
    args = parser.parse_args()
    if os.name != 'nt' or not 0.5 <= args.interval <= 3:
        parser.error('Windows required; interval must be between 0.5 and 3 seconds')
    try:
        result = run(args.samples, args.interval)
    except Exception:
        # Never serialize a raw exception, instance name or local file path.
        print(json.dumps({'status': 'error', 'reason': 'probe_api_failure'}))
        return 1
    print(json.dumps(result, indent=2))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
