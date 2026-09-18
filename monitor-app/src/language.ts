export type Language = 'zh' | 'en';
export const LANGUAGE_KEY = 'monitor-language';
export function validLanguage(value: unknown): value is Language {
  return value === 'zh' || value === 'en';
}
export interface LanguageBackend {
  read(): Promise<{language: string; error: string | null}>;
  write(language: Language, migrateOnly: boolean): Promise<string>;
}
export interface LanguageCache {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}
export class LanguagePersistence {
  private backend: LanguageBackend | null;
  private cache: LanguageCache;
  constructor(backend: LanguageBackend | null, cache: LanguageCache) {this.backend=backend;this.cache=cache;}
  private cached(): Language | null {
    try { const value = this.cache.getItem(LANGUAGE_KEY); return validLanguage(value) ? value : null; }
    catch { return null; }
  }
  private clearLegacy() { try { this.cache.removeItem(LANGUAGE_KEY); } catch { /* backend remains authoritative */ } }
  async load(): Promise<Language> {
    if (!this.backend) return this.cached() ?? 'zh';
    const saved = await this.backend.read();
    if (saved.error) throw new Error(saved.error);
    if (validLanguage(saved.language)) { this.clearLegacy(); return saved.language; }
    if (saved.language !== 'system') throw new Error('Invalid language setting');
    const legacy = this.cached();
    if (!legacy) return 'zh'; // Existing application default, not OS locale detection.
    const migrated = await this.backend.write(legacy, true);
    if (!validLanguage(migrated)) throw new Error('Invalid language response');
    this.clearLegacy();
    return migrated;
  }
  async save(language: Language): Promise<Language> {
    if (!validLanguage(language)) throw new Error('Invalid language');
    if (!this.backend) { this.cache.setItem(LANGUAGE_KEY, language); return language; }
    const saved = await this.backend.write(language, false);
    if (!validLanguage(saved)) throw new Error('Invalid language response');
    this.clearLegacy();
    return saved;
  }
}
