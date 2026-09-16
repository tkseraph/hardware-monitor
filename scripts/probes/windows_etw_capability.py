"""Explicit short-lived permission probe. No consumer, file output or privilege changes."""
import ctypes as c
import json
import os

class Guid(c.Structure):
    _fields_=[('a',c.c_uint32),('b',c.c_uint16),('d',c.c_uint16),('e',c.c_ubyte*8)]
class Wnode(c.Structure):
    _fields_=[('BufferSize',c.c_uint32),('ProviderId',c.c_uint32),('HistoricalContext',c.c_uint64),
              ('Timestamp',c.c_uint64),('Guid',Guid),('ClientContext',c.c_uint32),('Flags',c.c_uint32)]
class Properties(c.Structure):
    _fields_=[('Wnode',Wnode)]+[(name,c.c_uint32) for name in
        ['BufferSize','MinimumBuffers','MaximumBuffers','MaximumFileSize','LogFileMode','FlushTimer','EnableFlags','AgeLimit',
         'NumberOfBuffers','FreeBuffers','EventsLost','BuffersWritten','LogBuffersLost','RealTimeBuffersLost']]+[
        ('LoggerThreadId',c.c_void_p),('LogFileNameOffset',c.c_uint32),('LoggerNameOffset',c.c_uint32)]

def main():
    if os.name!='nt':raise SystemExit('Windows required')
    name='HardwareMonitor-StorageCapability'
    encoded=(name+'\0').encode('utf-16-le')
    buffer=c.create_string_buffer(c.sizeof(Properties)+len(encoded))
    props=c.cast(buffer,c.POINTER(Properties))
    props.contents.Wnode.BufferSize=len(buffer)
    props.contents.Wnode.ClientContext=1
    props.contents.Wnode.Flags=0x00020000
    props.contents.BufferSize=64
    props.contents.MinimumBuffers=2
    props.contents.MaximumBuffers=4
    props.contents.LogFileMode=0x02000000|0x00000100|0x10000000
    props.contents.EnableFlags=0x00000100
    props.contents.LoggerNameOffset=c.sizeof(Properties)
    c.memmove(c.addressof(buffer)+c.sizeof(Properties),encoded,len(encoded))
    api=c.WinDLL('advapi32.dll')
    api.StartTraceW.argtypes=[c.POINTER(c.c_uint64),c.c_wchar_p,c.POINTER(Properties)]
    api.StartTraceW.restype=c.c_uint32
    api.ControlTraceW.argtypes=[c.c_uint64,c.c_wchar_p,c.POINTER(Properties),c.c_uint32]
    api.ControlTraceW.restype=c.c_uint32
    handle=c.c_uint64()
    code=api.StartTraceW(c.byref(handle),name,props)
    result={'start_code':int(code),'started':code==0,'events_consumed':0,'file_logging':False,'privilege_changed':False}
    if code==0:
        try:
            pass  # Deliberately do not open a consumer or collect any event payload.
        finally:
            stopped=api.ControlTraceW(handle,None,props,1)
            result['stop_code']=int(stopped)
            if stopped!=0:result['session_requiring_cleanup']=name
    print(json.dumps(result))
    return 1 if code==0 and result['stop_code']!=0 else 0

if __name__=='__main__':raise SystemExit(main())
