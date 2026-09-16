export interface RuntimeInfo {
  platform: 'windows' | 'macos';
  terminate_process: boolean;
  launch_at_login: boolean;
  process_disk_io: boolean;
  primary_instance: boolean;
  effective_interval_ms: number;
}
