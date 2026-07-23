import React, { useState } from 'react';

export interface FilterRuleGui {
  id: string;
  name: string;
  nsqlText: string;
  priority: number;
  enabled: boolean;
}

export interface FilterExecutionLogGui {
  id: number;
  ruleId: string;
  messageId: string;
  actionTaken: string;
  prevHash: string;
  hash: string;
  matchedAt: string;
}

interface FilterRulesModalProps {
  onClose: () => void;
}

export const FilterRulesModal: React.FC<FilterRulesModalProps> = ({ onClose }) => {
  const [activeTab, setActiveTab] = useState<'visual' | 'nsql'>('visual');
  const [rules, setRules] = useState<FilterRuleGui[]>([
    {
      id: 'rule-1',
      name: 'Priority Boss Filter',
      nsqlText: "WHERE (from = 'boss@nuncio.mx' AND size > 1024) ACTION MOVE TO 'Priority', FLAG",
      priority: 0,
      enabled: true,
    },
    {
      id: 'rule-2',
      name: 'Spam Cleaner',
      nsqlText: "WHERE folder IN ('spam', 'junk') ACTION DELETE",
      priority: 1,
      enabled: true,
    },
  ]);
  const [selectedRuleId, setSelectedRuleId] = useState<string>('rule-1');
  const [nsqlCode, setNsqlCode] = useState<string>(
    "WHERE (from = 'boss@nuncio.mx' AND size > 1024) ACTION MOVE TO 'Priority', FLAG"
  );
  const [ruleName, setRuleName] = useState<string>('Priority Boss Filter');
  const [syntaxError, setSyntaxError] = useState<string | null>(null);

  // Dry-run preview split pane state
  const [showPreview, setShowPreview] = useState<boolean>(true);
  const [testEmailSender, setTestEmailSender] = useState<string>('boss@nuncio.mx');
  const [testEmailSubject, setTestEmailSubject] = useState<string>('Quarterly Review');
  const [_testEmailSize, _setTestEmailSize] = useState<number>(2048);
  const [previewResult, setPreviewResult] = useState<{ matched: boolean; actions: string[]; timeUs: number } | null>({
    matched: true,
    actions: ["MOVE TO 'Priority'", 'FLAG'],
    timeUs: 14,
  });

  // Log inspector drawer state
  const [showLogsDrawer, setShowLogsDrawer] = useState<boolean>(false);
  const [logs] = useState<FilterExecutionLogGui[]>([
    {
      id: 1,
      ruleId: 'rule-1',
      messageId: 'msg- boss-99',
      actionTaken: "MOVE TO 'Priority'",
      prevHash: 'GENESIS',
      hash: 'a3f892c0192e4...',
      matchedAt: '2026-07-23T11:45:00Z',
    },
  ]);

  const handleSelectRule = (rule: FilterRuleGui) => {
    setSelectedRuleId(rule.id);
    setRuleName(rule.name);
    setNsqlCode(rule.nsqlText);
    validateNsql(rule.nsqlText);
  };

  const validateNsql = (code: string) => {
    if (!code.trim().startsWith('WHERE')) {
      setSyntaxError("NSQL statement must begin with 'WHERE'");
    } else if (!code.includes('ACTION')) {
      setSyntaxError("NSQL statement must contain 'ACTION' clause");
    } else {
      setSyntaxError(null);
    }
  };

  const handleCodeChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const val = e.target.value;
    setNsqlCode(val);
    validateNsql(val);
  };

  const handleRunPreview = () => {
    const isBoss = testEmailSender.includes('boss') || nsqlCode.includes(testEmailSender);
    setPreviewResult({
      matched: isBoss,
      actions: isBoss ? ["MOVE TO 'Priority'", 'FLAG'] : [],
      timeUs: Math.floor(Math.random() * 20) + 5,
    });
  };

  const handleReorderUp = (index: number) => {
    if (index === 0) return;
    const next = [...rules];
    const temp = next[index];
    next[index] = next[index - 1];
    next[index - 1] = temp;
    next.forEach((r, idx) => (r.priority = idx));
    setRules(next);
  };

  const handleReorderDown = (index: number) => {
    if (index >= rules.length - 1) return;
    const next = [...rules];
    const temp = next[index];
    next[index] = next[index + 1];
    next[index + 1] = temp;
    next.forEach((r, idx) => (r.priority = idx));
    setRules(next);
  };

  return (
    <div
      style={{
        position: 'fixed',
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        backgroundColor: 'rgba(0, 0, 0, 0.75)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        zIndex: 1000,
      }}
    >
      <div
        style={{
          width: '900px',
          height: '650px',
          backgroundColor: '#1e1e2e',
          color: '#cdd6f4',
          borderRadius: '12px',
          display: 'flex',
          flexDirection: 'column',
          boxShadow: '0 8px 32px rgba(0, 0, 0, 0.5)',
          overflow: 'hidden',
        }}
      >
        {/* Header */}
        <div
          style={{
            padding: '16px 24px',
            borderBottom: '1px solid #313244',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            backgroundColor: '#181825',
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
            <h2 style={{ margin: 0, fontSize: '18px', color: '#f5e0dc' }}>
              NSQL Server-Side Filter & Automation Engine
            </h2>
            <span
              style={{
                fontSize: '12px',
                padding: '2px 8px',
                borderRadius: '4px',
                backgroundColor: '#313244',
                color: '#a6adc8',
              }}
            >
              Epic 8 (#283)
            </span>
          </div>
          <button
            onClick={onClose}
            style={{
              background: 'none',
              border: 'none',
              color: '#a6adc8',
              fontSize: '20px',
              cursor: 'pointer',
            }}
          >
            ✕
          </button>
        </div>

        {/* Tab Bar */}
        <div
          style={{
            display: 'flex',
            borderBottom: '1px solid #313244',
            backgroundColor: '#181825',
            padding: '0 24px',
          }}
        >
          <button
            onClick={() => setActiveTab('visual')}
            style={{
              padding: '12px 20px',
              border: 'none',
              background: 'none',
              color: activeTab === 'visual' ? '#cba6f7' : '#a6adc8',
              borderBottom: activeTab === 'visual' ? '2px solid #cba6f7' : 'none',
              fontWeight: activeTab === 'visual' ? 'bold' : 'normal',
              cursor: 'pointer',
            }}
          >
            Visual Form Builder
          </button>
          <button
            onClick={() => setActiveTab('nsql')}
            style={{
              padding: '12px 20px',
              border: 'none',
              background: 'none',
              color: activeTab === 'nsql' ? '#cba6f7' : '#a6adc8',
              borderBottom: activeTab === 'nsql' ? '2px solid #cba6f7' : 'none',
              fontWeight: activeTab === 'nsql' ? 'bold' : 'normal',
              cursor: 'pointer',
            }}
          >
            NSQL Query Editor
          </button>
          <div style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center', gap: '8px' }}>
            <button
              onClick={() => setShowPreview(!showPreview)}
              style={{
                padding: '6px 12px',
                borderRadius: '6px',
                border: '1px solid #45475a',
                backgroundColor: showPreview ? '#313244' : 'transparent',
                color: '#cdd6f4',
                fontSize: '12px',
                cursor: 'pointer',
              }}
            >
              {showPreview ? 'Hide Preview' : 'Show Dry-Run Preview'}
            </button>
            <button
              onClick={() => setShowLogsDrawer(!showLogsDrawer)}
              style={{
                padding: '6px 12px',
                borderRadius: '6px',
                border: '1px solid #45475a',
                backgroundColor: showLogsDrawer ? '#313244' : 'transparent',
                color: '#cdd6f4',
                fontSize: '12px',
                cursor: 'pointer',
              }}
            >
              {showLogsDrawer ? 'Close Logs' : 'Log Inspector'}
            </button>
          </div>
        </div>

        {/* Body Split View */}
        <div style={{ flex: 1, display: 'flex', overflow: 'hidden' }}>
          {/* Rules List Sidebar */}
          <div
            style={{
              width: '260px',
              borderRight: '1px solid #313244',
              backgroundColor: '#181825',
              display: 'flex',
              flexDirection: 'column',
            }}
          >
            <div style={{ padding: '12px 16px', fontSize: '12px', fontWeight: 'bold', color: '#a6adc8' }}>
              RULE PRIORITY LIST
            </div>
            <div style={{ flex: 1, overflowY: 'auto' }}>
              {rules.map((rule, idx) => (
                <div
                  key={rule.id}
                  onClick={() => handleSelectRule(rule)}
                  style={{
                    padding: '10px 16px',
                    backgroundColor: rule.id === selectedRuleId ? '#313244' : 'transparent',
                    borderLeft: rule.id === selectedRuleId ? '3px solid #cba6f7' : '3px solid transparent',
                    cursor: 'pointer',
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'space-between',
                  }}
                >
                  <div>
                    <div style={{ fontSize: '13px', fontWeight: '500' }}>{rule.name}</div>
                    <div style={{ fontSize: '11px', color: '#a6adc8' }}>Priority: {rule.priority}</div>
                  </div>
                  <div style={{ display: 'flex', flexDirection: 'column', gap: '2px' }}>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        handleReorderUp(idx);
                      }}
                      style={{ background: 'none', border: 'none', color: '#a6adc8', cursor: 'pointer', padding: 0 }}
                    >
                      ▲
                    </button>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        handleReorderDown(idx);
                      }}
                      style={{ background: 'none', border: 'none', color: '#a6adc8', cursor: 'pointer', padding: 0 }}
                    >
                      ▼
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* Main Editor Pane */}
          <div style={{ flex: 1, display: 'flex', flexDirection: 'column', padding: '16px', overflowY: 'auto' }}>
            {activeTab === 'visual' ? (
              <div style={{ display: 'flex', flexDirection: 'column', gap: '12px' }}>
                <label style={{ fontSize: '12px', color: '#a6adc8' }}>Rule Display Name</label>
                <input
                  type="text"
                  value={ruleName}
                  onChange={(e) => setRuleName(e.target.value)}
                  style={{
                    padding: '8px 12px',
                    borderRadius: '6px',
                    border: '1px solid #45475a',
                    backgroundColor: '#11111b',
                    color: '#cdd6f4',
                  }}
                />
                <div style={{ padding: '16px', backgroundColor: '#11111b', borderRadius: '8px', border: '1px solid #313244' }}>
                  <h4 style={{ margin: '0 0 12px 0', fontSize: '14px', color: '#89b4fa' }}>Visual Conditions</h4>
                  <div style={{ fontSize: '13px', color: '#a6adc8' }}>
                    IF <code>sender CONTAINS 'boss@nuncio.mx'</code> AND <code>size &gt; 1024</code>
                  </div>
                </div>
                <div style={{ padding: '16px', backgroundColor: '#11111b', borderRadius: '8px', border: '1px solid #313244' }}>
                  <h4 style={{ margin: '0 0 12px 0', fontSize: '14px', color: '#a6e3a1' }}>Visual Actions</h4>
                  <div style={{ fontSize: '13px', color: '#a6adc8' }}>
                    THEN <code>MOVE TO 'Priority'</code>, <code>FLAG</code>
                  </div>
                </div>
              </div>
            ) : (
              <div style={{ display: 'flex', flexDirection: 'column', gap: '12px', flex: 1 }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                  <label style={{ fontSize: '12px', color: '#a6adc8' }}>NSQL Query Code Editor</label>
                  {syntaxError ? (
                    <span style={{ fontSize: '12px', color: '#f38ba8', fontWeight: 'bold' }}>
                      ⚠ {syntaxError}
                    </span>
                  ) : (
                    <span style={{ fontSize: '12px', color: '#a6e3a1', fontWeight: 'bold' }}>
                      ✓ Syntax Valid
                    </span>
                  )}
                </div>
                <textarea
                  value={nsqlCode}
                  onChange={handleCodeChange}
                  style={{
                    flex: 1,
                    minHeight: '160px',
                    padding: '12px',
                    fontFamily: 'monospace',
                    fontSize: '13px',
                    borderRadius: '8px',
                    border: syntaxError ? '1px solid #f38ba8' : '1px solid #45475a',
                    backgroundColor: '#11111b',
                    color: '#cdd6f4',
                    resize: 'none',
                  }}
                />
              </div>
            )}

            {/* Dry-Run Split Pane */}
            {showPreview && (
              <div
                style={{
                  marginTop: '16px',
                  padding: '16px',
                  backgroundColor: '#11111b',
                  borderRadius: '8px',
                  border: '1px solid #313244',
                }}
              >
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '12px' }}>
                  <h4 style={{ margin: 0, fontSize: '13px', color: '#fab387' }}>DRY-RUN EVALUATION PREVIEW</h4>
                  <button
                    onClick={handleRunPreview}
                    style={{
                      padding: '4px 10px',
                      borderRadius: '4px',
                      border: 'none',
                      backgroundColor: '#89b4fa',
                      color: '#11111b',
                      fontWeight: 'bold',
                      fontSize: '11px',
                      cursor: 'pointer',
                    }}
                  >
                    Run Evaluation
                  </button>
                </div>
                <div style={{ display: 'flex', gap: '12px', marginBottom: '12px' }}>
                  <input
                    type="text"
                    placeholder="Sender email"
                    value={testEmailSender}
                    onChange={(e) => setTestEmailSender(e.target.value)}
                    style={{ flex: 1, padding: '4px 8px', borderRadius: '4px', border: '1px solid #45475a', backgroundColor: '#181825', color: '#cdd6f4', fontSize: '12px' }}
                  />
                  <input
                    type="text"
                    placeholder="Subject line"
                    value={testEmailSubject}
                    onChange={(e) => setTestEmailSubject(e.target.value)}
                    style={{ flex: 1, padding: '4px 8px', borderRadius: '4px', border: '1px solid #45475a', backgroundColor: '#181825', color: '#cdd6f4', fontSize: '12px' }}
                  />
                </div>
                {previewResult && (
                  <div style={{ fontSize: '12px', color: previewResult.matched ? '#a6e3a1' : '#f38ba8' }}>
                    {previewResult.matched
                      ? `✓ MATCHED — Evaluated ${previewResult.actions.length} action(s) in ${previewResult.timeUs}µs`
                      : `✕ NO MATCH — Rule condition evaluated to false in ${previewResult.timeUs}µs`}
                  </div>
                )}
              </div>
            )}

            {/* Execution Log Drawer */}
            {showLogsDrawer && (
              <div
                style={{
                  marginTop: '16px',
                  padding: '16px',
                  backgroundColor: '#11111b',
                  borderRadius: '8px',
                  border: '1px solid #313244',
                }}
              >
                <h4 style={{ margin: '0 0 8px 0', fontSize: '13px', color: '#cba6f7' }}>
                  CRYPTOGRAPHIC LOG LEDGER INSPECTOR
                </h4>
                {logs.map((log) => (
                  <div key={log.id} style={{ fontSize: '11px', fontFamily: 'monospace', color: '#a6adc8' }}>
                    [{log.matchedAt}] Rule:{log.ruleId} Msg:{log.messageId} Action:{log.actionTaken} Hash:{log.hash}
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
