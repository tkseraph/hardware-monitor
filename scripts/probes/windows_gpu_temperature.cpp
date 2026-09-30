// One-shot ADLX 1.5 source probe; no tuning, tracking, driver install or app DB writes.
// Build against the official SDK tag v1.5, commit d9f04a9bba022d6cf6333f005dd540b4ad19fb63.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <dxgi1_2.h>
#include <wrl/client.h>
#include <ADLX.h>
#include <ISystem2.h>
#include <IPerformanceMonitoring.h>
#include <cmath>
#include <iostream>
#include <string>
#include <vector>

template<class T> struct Ref {
    T* p = nullptr;
    Ref() = default;
    Ref(const Ref&) = delete;
    Ref& operator=(const Ref&) = delete;
    ~Ref() { if (p) p->Release(); }
    T** put() { return &p; }
    T* operator->() const { return p; }
};
struct Library {
    HMODULE module;
    ~Library() { if (module) FreeLibrary(module); }
};
std::string escaped(const char* text) {
    std::string out = "\"";
    if (text) for (size_t i = 0; text[i] && i < 256; ++i) {
        unsigned char c = static_cast<unsigned char>(text[i]);
        if (c == '"' || c == '\\') { out += '\\'; out += static_cast<char>(c); }
        else if (c < 32) out += ' ';
        else out += static_cast<char>(c);
    }
    return out + '"';
}
int failure(const char* state, int code) {
    std::cout << "{\"state\":" << escaped(state) << ",\"code\":" << code << "}\n";
    return 0; // Source availability is data, not a probe execution failure.
}
std::vector<LUID> amd_adapters() {
    std::vector<LUID> ids;
    Microsoft::WRL::ComPtr<IDXGIFactory1> factory;
    if (FAILED(CreateDXGIFactory1(IID_PPV_ARGS(&factory)))) return ids;
    for (UINT i = 0; i < 16; ++i) {
        Microsoft::WRL::ComPtr<IDXGIAdapter1> adapter;
        if (FAILED(factory->EnumAdapters1(i, &adapter))) break;
        DXGI_ADAPTER_DESC1 desc{};
        if (SUCCEEDED(adapter->GetDesc1(&desc)) && desc.VendorId == 0x1002 &&
            !(desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE)) ids.push_back(desc.AdapterLuid);
    }
    return ids;
}
void temperature(adlx::IADLXGPUMetrics* metrics, bool supported, bool hotspot) {
    adlx_double value = 0;
    if (metrics && supported && ADLX_SUCCEEDED(hotspot ? metrics->GPUHotspotTemperature(&value) :
        metrics->GPUTemperature(&value)) && std::isfinite(value) && value > 0 && value <= 150)
        std::cout << value;
    else std::cout << "null";
}
int main(int argc, char**) {
    if (argc != 1) return failure("invalid_arguments", 0);
    wchar_t system[MAX_PATH];
    UINT n = GetSystemDirectoryW(system, MAX_PATH);
    if (!n || n >= MAX_PATH) return failure("system_directory_error", GetLastError());
    // Fixed protected system path, never search the current directory for a DLL.
    std::wstring dll = std::wstring(system) + L"\\amdadlx64.dll";
    Library library{LoadLibraryExW(dll.c_str(), nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32)};
    if (!library.module) return failure("library_unavailable", GetLastError());
    auto query = reinterpret_cast<ADLXQueryFullVersion_Fn>(GetProcAddress(library.module, ADLX_QUERY_FULL_VERSION_FUNCTION_NAME));
    auto initialize = reinterpret_cast<ADLXInitialize_Fn>(GetProcAddress(library.module, ADLX_INIT_FUNCTION_NAME));
    auto terminate = reinterpret_cast<ADLXTerminate_Fn>(GetProcAddress(library.module, ADLX_TERMINATE_FUNCTION_NAME));
    if (!query || !initialize || !terminate) return failure("exports_unavailable", 0);
    adlx_uint64 version = 0;
    ADLX_RESULT code = query(&version);
    if (!ADLX_SUCCEEDED(code) || version != ADLX_FULL_VERSION) return failure("runtime_version_mismatch", code);
    adlx::IADLXSystem* system_services = nullptr; // Owned by ADLX; not a refcounted interface.
    code = initialize(ADLX_FULL_VERSION, &system_services);
    if (!ADLX_SUCCEEDED(code) || !system_services) return failure("initialize_failed", code);
    const auto adapters = amd_adapters();
    {
        Ref<adlx::IADLXPerformanceMonitoringServices> performance;
        Ref<adlx::IADLXGPUList> gpus;
        code = system_services->GetPerformanceMonitoringServices(performance.put());
        if (!ADLX_SUCCEEDED(code) || !performance.p) failure("monitoring_unavailable", code);
        else if (!ADLX_SUCCEEDED(code = system_services->GetGPUs(gpus.put())) || !gpus.p || gpus->Size() > 16)
            failure("gpu_inventory_unavailable", code);
        else {
            // Three low-frequency current observations; no StartMetricsTracking/history buffer.
            std::cout << "{\"state\":\"ok\",\"runtime_version\":\"1.5.0.124\",\"observations\":[";
            for (int sample = 0; sample < 3; ++sample) {
                if (sample) { Sleep(2000); std::cout << ','; }
                std::cout << "{\"gpus\":[";
                bool first = true;
                for (adlx_uint i = gpus->Begin(); i != gpus->End(); ++i) {
                    Ref<adlx::IADLXGPU> gpu;
                    if (!ADLX_SUCCEEDED(gpus->At(i, gpu.put())) || !gpu.p) continue;
                    if (!first) std::cout << ',';
                    first = false;
                    const char* name = nullptr;
                    gpu->Name(&name);
                    Ref<adlx::IADLXGPU2> gpu2;
                    ADLX_LUID luid{};
                    bool has_luid = ADLX_SUCCEEDED(gpu->QueryInterface(adlx::IADLXGPU2::IID(), reinterpret_cast<void**>(gpu2.put()))) &&
                        gpu2.p && ADLX_SUCCEEDED(gpu2->LUID(&luid));
                    int matches = 0;
                    for (const auto& id : adapters) if (has_luid && id.HighPart == luid.highPart && id.LowPart == luid.lowPart) ++matches;
                    Ref<adlx::IADLXGPUMetricsSupport> support;
                    adlx_bool edge = false, hotspot = false;
                    if (ADLX_SUCCEEDED(performance->GetSupportedGPUMetrics(gpu.p, support.put())) && support.p) {
                        if (!ADLX_SUCCEEDED(support->IsSupportedGPUTemperature(&edge))) edge = false;
                        if (!ADLX_SUCCEEDED(support->IsSupportedGPUHotspotTemperature(&hotspot))) hotspot = false;
                    }
                    Ref<adlx::IADLXGPUMetrics> metrics;
                    ADLX_RESULT read = performance->GetCurrentGPUMetrics(gpu.p, metrics.put());
                    std::cout << "{\"name\":" << escaped(name) << ",\"dxgi_luid_match\":" << (matches == 1 ? "true" : "false")
                        << ",\"edge_supported\":" << (edge ? "true" : "false") << ",\"hotspot_supported\":" << (hotspot ? "true" : "false")
                        << ",\"metrics_code\":" << read << ",\"edge_c\":";
                    temperature(ADLX_SUCCEEDED(read) ? metrics.p : nullptr, edge, false);
                    std::cout << ",\"hotspot_c\":";
                    temperature(ADLX_SUCCEEDED(read) ? metrics.p : nullptr, hotspot, true);
                    std::cout << '}';
                }
                std::cout << "]}";
            }
            std::cout << "]}\n";
        }
    } // Release all GPU/metrics interfaces before termination and library unload.
    ADLX_RESULT stopped = terminate();
    return ADLX_SUCCEEDED(stopped) ? 0 : 2;
}
