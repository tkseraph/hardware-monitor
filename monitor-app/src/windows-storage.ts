import type { SourceState } from './source-state';
export interface WindowsDisk {
  number: number; name: string; size_bytes: number; bus_type: number;
  kind: 'physical' | 'virtual' | 'unknown'; device_uid: string | null; identity_state: SourceState;
}
export interface WindowsVolume {
  alias: string; drive_letter: string | null; size_bytes: number | null;
  free_bytes: number | null; disk_numbers: number[]; mapping_state: SourceState;
}
export interface DiskHistorySeries { uid: string; name: string; size_bytes: number; created_at: number }
export interface WindowsStorageSnapshot {
  disks: WindowsDisk[]; volumes: WindowsVolume[]; volume_state: SourceState;
  history_series: DiskHistorySeries[]; registry_state: SourceState;
}
export interface WindowsDiskRate {
  number: number; device_uid: string | null; read_bps: number | null; write_bps: number | null; state: SourceState;
}
export interface WindowsThroughput { disks: WindowsDiskRate[]; unmapped_instances: number }

export function groupVolumes(disks: WindowsDisk[], volumes: WindowsVolume[]) {
  const known = new Set(disks.map(d => d.number));
  const dedicated: Record<number, WindowsVolume[]> = {};
  const shared: WindowsVolume[] = [];
  for (const volume of volumes) {
    const numbers = [...new Set(volume.disk_numbers)];
    if (volume.mapping_state === 'ok' && numbers.length === 1 && known.has(numbers[0])) {
      (dedicated[numbers[0]] ??= []).push(volume);
    } else shared.push(volume);
  }
  return { dedicated, shared };
}
export function volumeUsage(volume: WindowsVolume): {used: number; percent: number} | null {
  const total = volume.size_bytes, free = volume.free_bytes;
  if (total === null || free === null || !Number.isSafeInteger(total) || !Number.isSafeInteger(free) || total <= 0 || free < 0 || free > total) return null;
  return {used: total-free, percent: (total-free)/total*100};
}
export function rateForDisk(disk: WindowsDisk, rates?: WindowsThroughput | null): WindowsDiskRate | undefined {
  if (!disk.device_uid) return undefined;
  return rates?.disks.find(rate => rate.number === disk.number && rate.device_uid === disk.device_uid);
}
