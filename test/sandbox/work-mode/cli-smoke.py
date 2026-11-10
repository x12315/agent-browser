"""Exercise newly built fork binary only inside the disposable sandbox guest."""
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

binary = sys.argv[1]
session = f'work-mode-fork-{os.getpid()}'
config = Path(tempfile.mkdtemp())/'config.json'
config.write_text(json.dumps({'headed':True}))
checks=[]
started=time.time()
def run(args, success=True, executable=None, session_name=None):
    p=subprocess.run([executable or binary,'--config',str(config),'--session',session_name or session,'--json',*args],text=True,capture_output=True,timeout=40)
    try:r=json.loads(p.stdout)
    except ValueError:raise AssertionError({'args':args,'rc':p.returncode,'stdout':p.stdout,'stderr':p.stderr})
    responses=r if isinstance(r,list) else [r]
    print(json.dumps({'command':args,'returncode':p.returncode,'response':r,'stderr':p.stderr}),flush=True)
    assert (p.returncode==0)==success and all(item['success']==success for item in responses), (args,r,p.stderr)
    return r
try:
    run(['--work-mode','background','open','data:text/html,<title>Protected Background</title><h1>Protected Background</h1>'])
    run(['eval','globalThis.sentinel = "preserved"'])
    info=run(['session','info'])['data']
    assert info['runtime']['workModePolicyVersion']==1,info
    assert info['runtime']['workMode']=='background',info
    assert info['runtime']['effectiveMode']=='headless',info
    checks.append('native_cli_mode_overrides_default_and_inherits')
    run(['--work-mode','collaboration','eval','globalThis.sentinel = "bad"'],False)
    run(['--headed','true','open','about:blank'],False)
    run(['--no-auto-dialog','true','eval','globalThis.sentinel = "restart-bad"'],False)
    run(['--cdp','9222','eval','globalThis.sentinel = "attach-bad"'],False)
    value=run(['eval','globalThis.sentinel'])['data']
    assert value.get('result')=='preserved',value
    checks.append('mode_headed_daemon_config_and_external_attach_refused_without_page_loss')
    # The first command of a batch must be allowed to close before reconfiguration.
    run(['--work-mode','collaboration','batch','close'])
    checks.append('batch_close_releases_previous_mode')
    run(['--work-mode','collaboration','open','data:text/html,<title>Protected Collaboration</title><h1>Protected Collaboration</h1>'])
    info=run(['session','info'])['data']
    assert info['runtime']['workMode']=='collaboration',info
    assert info['runtime']['effectiveMode']=='headed',info
    code="ObjC.import('CoreGraphics'); const ws=ObjC.deepUnwrap(ObjC.castRefToObject($.CGWindowListCopyWindowInfo(1,0))); JSON.stringify(ws.filter(w=>w.kCGWindowLayer===0 && w.kCGWindowAlpha>0).map(w=>w.kCGWindowName));"
    deadline=time.monotonic()+10
    while True:
        windows=json.loads(subprocess.check_output(['/usr/bin/osascript','-l','JavaScript','-e',code],text=True))
        if 'Protected Collaboration' in windows or time.monotonic()>=deadline:break
        time.sleep(.2)
    assert 'Protected Collaboration' in windows,windows
    assert 'Protected Background' not in windows,windows
    checks.append('real_guest_collaboration_window_visible_background_not_visible')
    run(['--work-mode','background','close'])
    checks.append('explicit_close_allows_new_mode')
    legacy_session=session+'-legacy'
    def legacy(args):
        return run(['--headed','false',*args],executable='agent-browser',session_name=legacy_session)
    try:
        legacy(['open','data:text/html,<title>Legacy Preserved</title>'])
        legacy(['eval','globalThis.legacySentinel = "preserved"'])
        old_pid=legacy(['session','info'])['data']['pid']
        refused=run(['--work-mode','background','eval','globalThis.legacySentinel = "bad"'],False,session_name=legacy_session)
        assert 'workModePolicyVersion' in refused.get('error',''),refused
        assert legacy(['eval','globalThis.legacySentinel'])['data']['result']=='preserved'
        assert legacy(['session','info'])['data']['pid']==old_pid
        checks.append('unsupported_legacy_daemon_is_not_adopted_restarted_or_mutated')
    finally:
        legacy(['close'])
    print(json.dumps({'fork_cli_work_mode_smoke':'PASS','checks':checks,'version':subprocess.check_output([binary,'--version'],text=True).strip()}),flush=True)
except Exception:
    print('CANDIDATE_FAILURE_DIAGNOSTICS',flush=True)
    try:
        processes=subprocess.check_output(['/bin/ps','-axo','pid=,ppid=,stat=,command='],text=True)
        print('\n'.join(line for line in processes.splitlines() if 'agent-browser' in line or 'Chrome' in line)[-18000:],flush=True)
        reports=Path.home()/'Library/Logs/DiagnosticReports'
        for _ in range(3):
            hits=[p for p in reports.glob('*') if p.is_file() and p.stat().st_mtime>=started and 'agent-browser' in p.name]
            if hits:break
            time.sleep(1)
        for p in hits:
            print('CRASH_REPORT',p.name,flush=True)
            print(p.read_text(errors='replace')[:26000],flush=True)
    except Exception as error:print('diagnostic error',repr(error),flush=True)
    raise
finally:
    try:run(['close'])
    except Exception:pass
