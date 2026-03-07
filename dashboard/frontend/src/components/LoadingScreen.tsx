import type { LoadingStep } from '../types';

interface Props {
  steps: LoadingStep[];
}

function formatMs(ms: number | undefined): string {
  if (ms === undefined) return '';
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

function StepIcon({ status }: { status: LoadingStep['status'] }) {
  switch (status) {
    case 'pending':
      return <span className="step-icon pending">○</span>;
    case 'running':
      return (
        <span className="step-icon running">
          <span className="pulse-ring" />
        </span>
      );
    case 'done':
      return <span className="step-icon done">✓</span>;
    case 'error':
      return <span className="step-icon error">✗</span>;
  }
}

export default function LoadingScreen({ steps }: Props) {
  const allDone = steps.every(s => s.status === 'done');
  const hasError = steps.some(s => s.status === 'error');
  const runningIdx = steps.findIndex(s => s.status === 'running');
  const doneCount = steps.filter(s => s.status === 'done').length;

  return (
    <div className="loading-overlay">
      <div className="loading-card">
        {/* Header */}
        <div className="loading-header">
          <div className="loading-logo">🚢</div>
          <h1>Barquitos</h1>
          <p className="loading-subtitle">Ship Tracking Dashboard</p>
        </div>

        {/* Progress bar */}
        <div className="loading-progress-track">
          <div
            className={`loading-progress-fill ${hasError ? 'error' : ''} ${allDone ? 'complete' : ''}`}
            style={{ width: `${(doneCount / steps.length) * 100}%` }}
          />
        </div>

        {/* Step list */}
        <div className="loading-steps">
          {steps.map((step, i) => (
            <div
              key={i}
              className={`loading-step ${step.status} ${i === runningIdx ? 'active' : ''}`}
            >
              <StepIcon status={step.status} />
              <div className="step-content">
                <div className="step-label">{step.label}</div>
                {step.detail && (
                  <div className="step-detail">{step.detail}</div>
                )}
              </div>
              {step.durationMs !== undefined && (
                <div className="step-duration">{formatMs(step.durationMs)}</div>
              )}
            </div>
          ))}
        </div>

        {/* Footer message */}
        <div className="loading-footer">
          {hasError
            ? 'Connection failed — check that the server is running'
            : allDone
              ? 'Ready!'
              : 'Loading data from database…'}
        </div>
      </div>
    </div>
  );
}
