# Ritual dApp Frontend Skill

**Version:** 1.0.0

Build production-grade React/Next.js frontends for Ritual Chain dApps with async transaction state machines, on-chain event subscriptions, and wallet integration.

---

## When to Use

**Use this skill when:**
- Building a frontend for a Ritual Chain dApp
- Implementing async transaction tracking UI
- Adding wallet connection for Ritual Chain
- Creating hooks for precompile interactions
- Displaying job status, fee estimates, or streaming results
- Styling transaction state indicators

**Do NOT use when:**
- Building backend services (use contract interaction patterns instead)
- Writing smart contracts (use Solidity tooling)
- Working with non-Ritual EVM chains (adapt chain config)

---

## Tech Stack

| Layer | Technology | Purpose |
|-------|------------|---------|
| Framework | Next.js 14+ (App Router) | SSR, routing, API routes |
| Chain | wagmi v2 + viem v2 | Wallet, contracts, events |
| State | Zustand v4 | Transaction state machine |
| Async | React Query v5 | Polling, caching |
| Styling | Tailwind v3 | Dark-mode-first design |

**Core dependencies:**
```json
{
  "next": "^14.2.0",
  "wagmi": "^2.12.0",
  "viem": "^2.21.0",
  "@tanstack/react-query": "^5.59.0",
  "zustand": "^4.5.0"
}
```

---

## Quick Start Pattern

### 1. Setup Chain & Providers

**File: `lib/chain.ts`**
```typescript
import { defineChain } from 'viem';

export const ritualChain = defineChain({
  id: 1979,
  name: 'Ritual',
  nativeCurrency: { name: 'RIT', symbol: 'RIT', decimals: 18 },
  rpcUrls: {
    default: { http: ['https://rpc.ritual.network'] },
  },
  blockExplorers: {
    default: { name: 'Ritual Explorer', url: 'https://explorer.ritual.network' },
  },
});
```

**File: `lib/wagmi.ts`**
```typescript
import { createConfig, http } from 'wagmi';
import { injected, walletConnect } from 'wagmi/connectors';
import { ritualChain } from './chain';

export const wagmiConfig = createConfig({
  chains: [ritualChain],
  connectors: [
    injected(),
    walletConnect({ projectId: process.env.NEXT_PUBLIC_WC_PROJECT_ID! }),
  ],
  transports: {
    [ritualChain.id]: http('https://rpc.ritual.network'),
  },
});
```

**File: `app/providers.tsx`**
```typescript
'use client';
import { WagmiProvider } from 'wagmi';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { wagmiConfig } from '@/lib/wagmi';
import { useState } from 'react';

export function Providers({ children }: { children: React.ReactNode }) {
  const [queryClient] = useState(() => new QueryClient({
    defaultOptions: {
      queries: { staleTime: 4_000, refetchInterval: 8_000 },
    },
  }));

  return (
    <WagmiProvider config={wagmiConfig}>
      <QueryClientProvider client={queryClient}>
        {children}
      </QueryClientProvider>
    </WagmiProvider>
  );
}
```

### 2. Add Wallet Connection

**File: `components/ConnectWallet.tsx`**
```tsx
'use client';
import { useAccount, useConnect, useDisconnect, useBalance } from 'wagmi';
import { ritualChain } from '@/lib/chain';

export function ConnectWallet() {
  const { address, isConnected, chain } = useAccount();
  const { connect, connectors, isPending } = useConnect();
  const { disconnect } = useDisconnect();
  const { data: balance } = useBalance({ address, chainId: ritualChain.id });

  if (isConnected && address) {
    const wrongChain = chain?.id !== ritualChain.id;
    return (
      <div className="flex items-center gap-3">
        {wrongChain && <span className="text-xs text-amber-400">Switch to Ritual Chain</span>}
        <div className="bg-gray-900 border border-gray-700 rounded-lg px-3 py-2 flex items-center gap-2">
          <div className="w-2 h-2 rounded-full bg-green-400 animate-pulse" />
          <span className="text-sm font-mono">{address.slice(0, 6)}...{address.slice(-4)}</span>
          {balance && <span className="text-xs text-gray-500">{parseFloat(balance.formatted).toFixed(3)} {balance.symbol}</span>}
        </div>
        <button onClick={() => disconnect()} className="text-xs text-gray-500 hover:text-gray-300">
          Disconnect
        </button>
      </div>
    );
  }

  return (
    <div className="flex gap-2">
      {connectors.map((connector) => (
        <button
          key={connector.uid}
          onClick={() => connect({ connector })}
          disabled={isPending}
          className="px-4 py-2 bg-transparent border border-green-500 text-green-400 rounded-lg hover:bg-green-500/10 disabled:opacity-50"
        >
          {isPending ? 'Connecting...' : `Connect ${connector.name}`}
        </button>
      ))}
    </div>
  );
}
```

---

## Async Transaction State Machine

**Critical:** Ritual precompile calls are **asynchronous**. A transaction progresses through up to 9 states.

### State Flow Diagram

```
SUBMITTING → PENDING_COMMITMENT → COMMITTED → EXECUTOR_PROCESSING 
  ↓              ↓                    ↓              ↓
FAILED        EXPIRED              FAILED        RESULT_READY → PENDING_SETTLEMENT → SETTLED
                                                    ↓                ↓
                                                  FAILED          FAILED
```

### TypeScript Types

**File: `types/asyncTx.ts`**
```typescript
export type AsyncTxStatus =
  | 'SUBMITTING'
  | 'PENDING_COMMITMENT'
  | 'COMMITTED'
  | 'EXECUTOR_PROCESSING'
  | 'RESULT_READY'
  | 'PENDING_SETTLEMENT'
  | 'SETTLED'
  | 'FAILED'
  | 'EXPIRED';

export interface AsyncTxState {
  status: AsyncTxStatus;
  txHash?: `0x${string}`;
  jobId?: bigint;
  error?: string;
  // ... state-specific fields
}

export function isTerminalState(status: AsyncTxStatus): boolean {
  return status === 'SETTLED' || status === 'FAILED' || status === 'EXPIRED';
}

export function canTransition(from: AsyncTxStatus, to: AsyncTxStatus): boolean {
  const validTransitions: Record<AsyncTxStatus, AsyncTxStatus[]> = {
    SUBMITTING: ['PENDING_COMMITMENT', 'FAILED'],
    PENDING_COMMITMENT: ['COMMITTED', 'EXPIRED', 'FAILED'],
    COMMITTED: ['EXECUTOR_PROCESSING', 'FAILED'],
    EXECUTOR_PROCESSING: ['RESULT_READY', 'FAILED'],
    RESULT_READY: ['PENDING_SETTLEMENT', 'FAILED'],
    PENDING_SETTLEMENT: ['SETTLED', 'FAILED'],
    SETTLED: [],
    FAILED: [],
    EXPIRED: [],
  };
  return validTransitions[from]?.includes(to) ?? false;
}
```

### State Store (Zustand)

**File: `stores/asyncTxStore.ts`**
```typescript
import { create } from 'zustand';
import { subscribeWithSelector } from 'zustand/middleware';

interface TrackedTransaction {
  id: string;
  precompileType: 'http' | 'llm' | 'agent' | 'longhttp' | 'image';
  state: AsyncTxState;
  createdAt: number;
  updatedAt: number;
  label?: string;
}

interface AsyncTxStore {
  transactions: Record<string, TrackedTransaction>;
  addTransaction: (id: string, type: TrackedTransaction['precompileType'], label?: string) => void;
  updateState: (id: string, newState: AsyncTxState) => void;
  removeTransaction: (id: string) => void;
  getActiveTransactions: () => TrackedTransaction[];
}

export const useAsyncTxStore = create<AsyncTxStore>()(
  subscribeWithSelector((set, get) => ({
    transactions: {},
    
    addTransaction: (id, precompileType, label) => {
      set((state) => ({
        transactions: {
          ...state.transactions,
          [id]: {
            id,
            precompileType,
            state: { status: 'SUBMITTING' },
            createdAt: Date.now(),
            updatedAt: Date.now(),
            label,
          },
        },
      }));
    },
    
    updateState: (id, newState) => {
      set((state) => {
        const existing = state.transactions[id];
        if (!existing) return state;
        return {
          transactions: {
            ...state.transactions,
            [id]: { ...existing, state: newState, updatedAt: Date.now() },
          },
        };
      });
    },
    
    removeTransaction: (id) => {
      set((state) => {
        const { [id]: _, ...rest } = state.transactions;
        return { transactions: rest };
      });
    },
    
    getActiveTransactions: () => {
      return Object.values(get().transactions).filter(
        (tx) => !isTerminalState(tx.state.status)
      );
    },
  }))
);
```

---

## Key Contract Addresses

**File: `lib/contracts.ts`**
```typescript
export const RITUAL_ADDRESSES = {
  PRECOMPILE: {
    HTTP_CALL: '0x0000000000000000000000000000000000000801',
    LLM: '0x0000000000000000000000000000000000000802',
    LONG_HTTP: '0x0000000000000000000000000000000000000805',
    AGENT_CALL: '0x0000000000000000000000000000000000000808',
    IMAGE_CALL: '0x0000000000000000000000000000000000000818',
  },
  NATIVE: {
    WALLET: '0x532F0dF0896F353d8C3DD8cc134e8129DA2a3948',
    ASYNC_JOB_TRACKER: '0xC069FFCa0389f44eCA2C626e55491b0ab045AEF5',
    TEE_SERVICE_REGISTRY: '0x9644e8562cE0Fe12b4deeC4163c064A8862Bf47F',
  },
} as const;

export const ASYNC_JOB_TRACKER_ABI = [
  {
    type: 'event',
    name: 'JobAdded',
    inputs: [
      { name: 'jobId', type: 'uint256', indexed: true },
      { name: 'requester', type: 'address', indexed: true },
      { name: 'executor', type: 'address', indexed: false },
    ],
  },
  {
    type: 'event',
    name: 'JobFulfilled',
    inputs: [
      { name: 'jobId', type: 'uint256', indexed: true },
      { name: 'resultHash', type: 'bytes32', indexed: false },
    ],
  },
  {
    type: 'event',
    name: 'JobDelivered',
    inputs: [
      { name: 'jobId', type: 'uint256', indexed: true },
      { name: 'deliveryTxHash', type: 'bytes32', indexed: false },
    ],
  },
] as const;
```

---

## Event Subscription Hook

**File: `hooks/useAsyncJobEvents.ts`**
```typescript
import { useWatchContractEvent, useAccount } from 'wagmi';
import { useAsyncTxStore } from '@/stores/asyncTxStore';
import { RITUAL_ADDRESSES, ASYNC_JOB_TRACKER_ABI } from '@/lib/contracts';

export function useAsyncJobEvents({ txId, enabled = true }: { txId: string; enabled?: boolean }) {
  const { address } = useAccount();
  const updateState = useAsyncTxStore((s) => s.updateState);
  const getTransaction = useAsyncTxStore((s) => s.getTransaction);

  // Watch JobAdded (executor commits)
  useWatchContractEvent({
    address: RITUAL_ADDRESSES.NATIVE.ASYNC_JOB_TRACKER,
    abi: ASYNC_JOB_TRACKER_ABI,
    eventName: 'JobAdded',
    args: { requester: address },
    enabled: enabled && !!address,
    onLogs: (logs) => {
      for (const log of logs) {
        const { jobId, executor } = log.args;
        const tx = getTransaction(txId);
        if (!tx || tx.state.status !== 'PENDING_COMMITMENT') continue;
        
        updateState(txId, {
          status: 'COMMITTED',
          txHash: tx.state.txHash,
          jobId: jobId!,
          executor: executor as `0x${string}`,
          committedBlock: Number(log.blockNumber),
        });
      }
    },
  });

  // Watch JobFulfilled (result ready)
  useWatchContractEvent({
    address: RITUAL_ADDRESSES.NATIVE.ASYNC_JOB_TRACKER,
    abi: ASYNC_JOB_TRACKER_ABI,
    eventName: 'JobFulfilled',
    enabled: enabled && !!address,
    onLogs: (logs) => {
      for (const log of logs) {
        const { jobId, resultHash } = log.args;
        const tx = getTransaction(txId);
        if (!tx) continue;
        
        if (tx.state.status === 'COMMITTED' || tx.state.status === 'EXECUTOR_PROCESSING') {
          updateState(txId, {
            status: 'RESULT_READY',
            txHash: tx.state.txHash,
            jobId: jobId!,
            resultHash: resultHash as `0x${string}`,
          });
        }
      }
    },
  });

  // Watch JobDelivered (settled)
  useWatchContractEvent({
    address: RITUAL_ADDRESSES.NATIVE.ASYNC_JOB_TRACKER,
    abi: ASYNC_JOB_TRACKER_ABI,
    eventName: 'JobDelivered',
    enabled: enabled && !!address,
    onLogs: (logs) => {
      for (const log of logs) {
        const { jobId, deliveryTxHash } = log.args;
        const tx = getTransaction(txId);
        if (!tx || tx.state.status !== 'RESULT_READY') continue;
        
        updateState(txId, {
          status: 'PENDING_SETTLEMENT',
          txHash: tx.state.txHash,
          jobId: jobId!,
          deliveryTxHash: deliveryTxHash as `0x${string}`,
        });
      }
    },
  });
}
```

---

## Transaction Patterns

### Pattern 1: HTTP Call (Simple Async)

**File: `hooks/useHTTPCall.ts`**
```typescript
import { useWriteContract, useAccount, useId, useCallback } from 'wagmi';
import { useAsyncTxStore } from '@/stores/asyncTxStore';
import { useAsyncJobEvents } from './useAsyncJobEvents';

interface UseHTTPCallOptions {
  url: string;
  method?: 'GET' | 'POST';
  headers?: Record<string, string>;
  body?: string;
  executor: `0x${string}`;
  ttl?: bigint;
  label?: string;
}

export function useHTTPCall() {
  const txId = useId();
  const { writeContractAsync } = useWriteContract();
  const addTransaction = useAsyncTxStore((s) => s.addTransaction);
  const updateState = useAsyncTxStore((s) => s.updateState);
  const getTransaction = useAsyncTxStore((s) => s.getTransaction);

  useAsyncJobEvents({ txId, enabled: !!getTransaction(txId) });

  const submit = useCallback(
    async (options: UseHTTPCallOptions) => {
      addTransaction(txId, 'http', options.label);

      try {
        const hash = await writeContractAsync({
          address: '0xaB340eEBdEdA29Af986c9239c569b23287C83cfE', // HTTP Helper
          abi: httpHelperAbi,
          functionName: options.method === 'POST' ? 'postRequest' : 'getRequest',
          args: [
            0n, // targetBlockNumber
            options.executor,
            '0x', // secrets
            options.ttl ?? 100n,
            [], // secretSignatures
            options.url,
            Object.keys(options.headers ?? {}),
            Object.values(options.headers ?? {}),
            ...(options.method === 'POST' && options.body ? [options.body] : []),
          ],
          gas: 2_000_000n,
        });

        updateState(txId, {
          status: 'PENDING_COMMITMENT',
          txHash: hash,
          submittedAt: Date.now(),
          ttlBlocks: Number(options.ttl ?? 100n),
        });

        return hash;
      } catch (err) {
        updateState(txId, {
          status: 'FAILED',
          error: err instanceof Error ? err.message : 'Transaction failed',
          errorCategory: 'wallet',
          failedAt: 'SUBMITTING',
        });
        throw err;
      }
    },
    [txId, writeContractAsync, addTransaction, updateState]
  );

  return { txId, submit, state: getTransaction(txId)?.state ?? null };
}

const httpHelperAbi = [
  {
    type: 'function',
    name: 'getRequest',
    inputs: [
      { name: 'targetBlockNumber', type: 'uint256' },
      { name: 'executor', type: 'address' },
      { name: 'secrets', type: 'bytes' },
      { name: 'ttl', type: 'uint256' },
      { name: 'secretSignatures', type: 'bytes[]' },
      { name: 'url', type: 'string' },
      { name: 'headerKeys', type: 'string[]' },
      { name: 'headerValues', type: 'string[]' },
    ],
    outputs: [],
    stateMutability: 'nonpayable',
  },
] as const;
```

### Pattern 2: Agent Call (Two-Phase)

**File: `hooks/useAgentCall.ts`**
```typescript
import { useWriteContract, useId, useCallback } from 'wagmi';
import { encodeAbiParameters } from 'viem';
import { useAsyncTxStore } from '@/stores/asyncTxStore';
import { useAsyncJobEvents } from './useAsyncJobEvents';

interface UseAgentCallOptions {
  executor: `0x${string}`;
  prompt: string;
  tools?: string[];
  maxIterations?: number;
  ttl?: bigint;
  label?: string;
}

export function useAgentCall() {
  const txId = useId();
  const { writeContractAsync } = useWriteContract();
  const addTransaction = useAsyncTxStore((s) => s.addTransaction);
  const updateState = useAsyncTxStore((s) => s.updateState);
  const getTransaction = useAsyncTxStore((s) => s.getTransaction);

  useAsyncJobEvents({ txId, enabled: !!getTransaction(txId) });

  const submit = useCallback(
    async (options: UseAgentCallOptions) => {
      addTransaction(txId, 'agent', options.label);

      try {
        const AGENT_PRECOMPILE = '0x0000000000000000000000000000000000000808';
        const encoded = encodeAbiParameters(
          [
            { name: 'executor', type: 'address' },
            { name: 'encryptedSecrets', type: 'bytes[]' },
            { name: 'ttl', type: 'uint256' },
            { name: 'secretSignature', type: 'bytes[]' },
            { name: 'userPublicKey', type: 'bytes' },
            { name: 'pollIntervalBlocks', type: 'uint64' },
            { name: 'maxPollBlock', type: 'uint64' },
            { name: 'taskIdMarker', type: 'string' },
            { name: 'deliveryTarget', type: 'address' },
            { name: 'deliverySelector', type: 'bytes4' },
            { name: 'deliveryGasLimit', type: 'uint256' },
            { name: 'deliveryMaxFeePerGas', type: 'uint256' },
            { name: 'deliveryMaxPriorityFeePerGas', type: 'uint256' },
            { name: 'deliveryValue', type: 'uint256' },
            { name: 'prompt', type: 'string' },
            { name: 'tools', type: 'string[]' },
            { name: 'maxIterations', type: 'uint16' },
            { name: 'maxToolCalls', type: 'uint16' },
            { name: 'maxTokens', type: 'uint32' },
            { name: 'temperatureScaled', type: 'uint16' },
            { name: 'piiEnabled', type: 'bool' },
          ],
          [
            options.executor,
            [],
            options.ttl ?? 200n,
            [],
            '0x',
            5n,
            1000n,
            'AGENT_TASK',
            '0x0000000000000000000000000000000000000000',
            '0x00000000',
            3_000_000n,
            1_000_000_000n,
            100_000_000n,
            0n,
            options.prompt,
            options.tools ?? [],
            options.maxIterations ?? 10,
            20,
            1024,
            70,
            false,
          ]
        );

        const hash = await writeContractAsync({
          address: AGENT_PRECOMPILE,
          abi: [{ type: 'fallback', stateMutability: 'nonpayable' }],
          data: encoded,
          gas: 3_000_000n,
        });

        updateState(txId, {
          status: 'PENDING_COMMITMENT',
          txHash: hash,
          submittedAt: Date.now(),
          ttlBlocks: Number(options.ttl ?? 200n),
        });

        return hash;
      } catch (err) {
        updateState(txId, {
          status: 'FAILED',
          error: err instanceof Error ? err.message : 'Agent call failed',
          errorCategory: 'contract',
          failedAt: 'SUBMITTING',
        });
        throw err;
      }
    },
    [txId, writeContractAsync, addTransaction, updateState]
  );

  return { txId, submit, state: getTransaction(txId)?.state ?? null };
}
```

---

## UI Components

### AsyncTransactionStatus

**File: `components/AsyncTransactionStatus.tsx`**
```tsx
'use client';
import type { AsyncTxState } from '@/types/asyncTx';

const STATUS_CONFIG: Record<string, { label: string; icon: string; color: string; bgColor: string }> = {
  SUBMITTING: { label: 'Submitting', icon: '↗', color: 'text-blue-400', bgColor: 'bg-blue-400/10' },
  PENDING_COMMITMENT: { label: 'Awaiting Executor', icon: '◎', color: 'text-yellow-400', bgColor: 'bg-yellow-400/10' },
  COMMITTED: { label: 'Executor Committed', icon: '✓', color: 'text-cyan-400', bgColor: 'bg-cyan-400/10' },
  EXECUTOR_PROCESSING: { label: 'Processing', icon: '⟳', color: 'text-green-400', bgColor: 'bg-green-400/10' },
  RESULT_READY: { label: 'Result Ready', icon: '◆', color: 'text-lime-400', bgColor: 'bg-lime-400/10' },
  PENDING_SETTLEMENT: { label: 'Settling', icon: '⧖', color: 'text-green-400', bgColor: 'bg-green-400/10' },
  SETTLED: { label: 'Settled', icon: '✔', color: 'text-green-400', bgColor: 'bg-green-400/10' },
  FAILED: { label: 'Failed', icon: '✕', color: 'text-red-400', bgColor: 'bg-red-400/10' },
  EXPIRED: { label: 'Expired', icon: '⏱', color: 'text-gray-400', bgColor: 'bg-gray-400/10' },
};

export function AsyncTransactionStatus({ state }: { state: AsyncTxState }) {
  const config = STATUS_CONFIG[state.status];
  if (!config) return null;

  return (
    <div className={`rounded-lg border border-gray-800 ${config.bgColor} p-4`}>
      <div className="flex items-center gap-3">
        <span className={`text-lg ${config.color}`}>{config.icon}</span>
        <div className="flex-1">
          <span className={`text-sm font-medium ${config.color}`}>{config.label}</span>
          {'txHash' in state && state.txHash && (
            <span className="text-xs text-gray-600 font-mono ml-2">
              {state.txHash.slice(0, 10)}...
            </span>
          )}
          {state.status === 'FAILED' && 'error' in state && (
            <p className="text-xs text-red-300/70 mt-1">{state.error}</p>
          )}
        </div>
        {'jobId' in state && state.jobId !== undefined && (
          <span className="text-xs text-gray-600 font-mono">
            Job #{state.jobId.toString()}
          </span>
        )}
      </div>
    </div>
  );
}
```

### RitualWalletCard

**File: `components/RitualWalletCard.tsx`**
```tsx
'use client';
import { useReadContract, useWriteContract, useAccount } from 'wagmi';
import { useState } from 'react';
import { parseEther, formatEther } from 'viem';

const RITUAL_WALLET = '0x532F0dF0896F353d8C3DD8cc134e8129DA2a3948';

const ritualWalletAbi = [
  { type: 'function', name: 'balanceOf', inputs: [{ type: 'address' }], outputs: [{ type: 'uint256' }], stateMutability: 'view' },
  { type: 'function', name: 'deposit', inputs: [{ name: 'lockDuration', type: 'uint256' }], outputs: [], stateMutability: 'payable' },
  { type: 'function', name: 'withdraw', inputs: [{ name: 'amount', type: 'uint256' }], outputs: [], stateMutability: 'nonpayable' },
] as const;

export function RitualWalletCard() {
  const { address } = useAccount();
  const { data: balance } = useReadContract({
    address: RITUAL_WALLET,
    abi: ritualWalletAbi,
    functionName: 'balanceOf',
    args: address ? [address] : undefined,
    query: { enabled: !!address, refetchInterval: 12_000 },
  });
  const { writeContractAsync } = useWriteContract();
  const [amount, setAmount] = useState('');
  const [mode, setMode] = useState<'deposit' | 'withdraw'>('deposit');

  const handleSubmit = async () => {
    if (!amount) return;
    if (mode === 'deposit') {
      await writeContractAsync({
        address: RITUAL_WALLET,
        abi: ritualWalletAbi,
        functionName: 'deposit',
        args: [100n],
        value: parseEther(amount),
      });
    } else {
      await writeContractAsync({
        address: RITUAL_WALLET,
        abi: ritualWalletAbi,
        functionName: 'withdraw',
        args: [parseEther(amount)],
      });
    }
    setAmount('');
  };

  const balanceFormatted = balance ? formatEther(balance) : '0';

  return (
    <div className="bg-gray-900 border border-gray-800 rounded-xl p-6">
      <h3 className="text-sm font-medium text-gray-400 uppercase mb-4">Ritual Wallet</h3>
      <p className="text-3xl font-bold font-mono mb-6">{parseFloat(balanceFormatted).toFixed(4)} RIT</p>
      
      <div className="flex gap-1 mb-3">
        <button
          onClick={() => setMode('deposit')}
          className={`flex-1 py-1.5 text-xs font-medium rounded-md ${
            mode === 'deposit' ? 'bg-green-500/20 text-green-400 border border-green-500/30' : 'text-gray-500'
          }`}
        >
          Deposit
        </button>
        <button
          onClick={() => setMode('withdraw')}
          className={`flex-1 py-1.5 text-xs font-medium rounded-md ${
            mode === 'withdraw' ? 'bg-amber-500/20 text-amber-400 border border-amber-500/30' : 'text-gray-500'
          }`}
        >
          Withdraw
        </button>
      </div>

      <div className="flex gap-2">
        <input
          type="number"
          value={amount}
          onChange={(e) => setAmount(e.target.value)}
          placeholder="Amount in RIT"
          className="flex-1 bg-black border border-gray-700 rounded-lg px-3 py-2 text-sm font-mono focus:border-green-500/50 focus:outline-none"
        />
        <button
          onClick={handleSubmit}
          disabled={!amount}
          className="px-4 py-2 bg-green-500/10 border border-green-500/30 text-green-400 rounded-lg text-sm hover:bg-green-500/20 disabled:opacity-40"
        >
          {mode === 'deposit' ? 'Deposit' : 'Withdraw'}
        </button>
      </div>
    </div>
  );
}
```

---

## Error Handling

**File: `lib/errors.ts`**
```typescript
export type ErrorCategory = 'wallet' | 'contract' | 'async' | 'network';

export interface RitualError {
  category: ErrorCategory;
  code: string;
  message: string;
  userMessage: string;
  recoverable: boolean;
  suggestion?: string;
}

export function categorizeError(error: unknown): RitualError {
  const message = error instanceof Error ? error.message : String(error);
  const lower = message.toLowerCase();

  if (lower.includes('user rejected') || lower.includes('user denied')) {
    return {
      category: 'wallet',
      code: 'USER_REJECTED',
      message,
      userMessage: 'Transaction was rejected in your wallet.',
      recoverable: true,
      suggestion: 'Try again and confirm the transaction.',
    };
  }

  if (lower.includes('insufficient funds')) {
    return {
      category: 'wallet',
      code: 'INSUFFICIENT_FUNDS',
      message,
      userMessage: 'Not enough RIT to cover this transaction.',
      recoverable: true,
      suggestion: 'Deposit more RIT to your wallet.',
    };
  }

  if (lower.includes('job expired') || lower.includes('ttl exceeded')) {
    return {
      category: 'async',
      code: 'JOB_EXPIRED',
      message,
      userMessage: 'The async job expired before an executor could process it.',
      recoverable: true,
      suggestion: 'Increase the TTL or try again during lower network activity.',
    };
  }

  return {
    category: 'network',
    code: 'UNKNOWN',
    message,
    userMessage: 'An unexpected error occurred.',
    recoverable: false,
  };
}
```

---

## Fee Estimation

**File: `hooks/useHTTPFeeEstimate.ts`**
```typescript
import { useGasPrice } from 'wagmi';
import { useMemo } from 'react';

const FEE_CONSTANTS = {
  HTTP_BASE_GAS: 200_000n,
  DEFAULT_GAS_PRICE: 20_000_000_000n, // 20 gwei
  DEFAULT_LOCK_DURATION: 100n,
};

export function useHTTPFeeEstimate() {
  const { data: gasPrice } = useGasPrice();

  return useMemo(() => {
    const price = gasPrice ?? FEE_CONSTANTS.DEFAULT_GAS_PRICE;
    const baseFee = FEE_CONSTANTS.HTTP_BASE_GAS * price;
    const executorFee = baseFee / 10n; // 10% executor premium
    const totalFee = baseFee + executorFee;

    return {
      baseFee,
      executorFee,
      totalFee,
      lockDuration: FEE_CONSTANTS.DEFAULT_LOCK_DURATION,
      gasPrice: price,
      isLoading: !gasPrice,
    };
  }, [gasPrice]);
}
```

---

## Complete Page Example

**File: `app/http-call/page.tsx`**
```tsx
'use client';
import { useState } from 'react';
import { ConnectWallet } from '@/components/ConnectWallet';
import { RitualWalletCard } from '@/components/RitualWalletCard';
import { AsyncTransactionStatus } from '@/components/AsyncTransactionStatus';
import { useHTTPCall } from '@/hooks/useHTTPCall';
import { useAccount } from 'wagmi';

export default function HTTPCallPage() {
  const { isConnected } = useAccount();
  const { txId, submit, state } = useHTTPCall();
  const [url, setUrl] = useState('https://api.coingecko.com/api/v3/ping');

  const handleSubmit = async () => {
    await submit({
      url,
      method: 'GET',
      executor: '0x...', // executor address
      ttl: 100n,
      label: 'API Health Check',
    });
  };

  return (
    <div className="min-h-screen bg-black text-white">
      <header className="border-b border-gray-800 px-6 py-4">
        <div className="flex items-center justify-between max-w-5xl mx-auto">
          <h1 className="text-lg font-semibold">Ritual HTTP Call</h1>
          <ConnectWallet />
        </div>
      </header>

      <main className="max-w-5xl mx-auto px-6 py-12 grid grid-cols-3 gap-8">
        <div className="col-span-2 space-y-6">
          <div className="bg-gray-900 border border-gray-800 rounded-xl p-6">
            <label className="block text-xs text-gray-500 uppercase mb-2">URL</label>
            <input
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              className="w-full bg-black border border-gray-700 rounded-lg px-4 py-3 text-sm font-mono focus:border-green-500/50 focus:outline-none"
            />
            <button
              onClick={handleSubmit}
              disabled={!isConnected || !url}
              className="mt-4 w-full py-3 bg-transparent border border-green-500 text-green-400 rounded-lg font-medium hover:bg-green-500/10 disabled:opacity-40"
            >
              Submit HTTP Call
            </button>
          </div>

          {state && <AsyncTransactionStatus state={state} />}
        </div>

        <aside>
          <RitualWalletCard />
        </aside>
      </main>
    </div>
  );
}
```

---

## Quick Reference

| Task | Hook/Component | Key Props |
|------|----------------|-----------|
| Connect wallet | `<ConnectWallet />` | — |
| Track async tx | `useAsyncJobEvents({ txId })` | txId |
| Show tx status | `<AsyncTransactionStatus state={...} />` | AsyncTxState |
| Wallet balance | `<RitualWalletCard />` | — |
| HTTP call | `useHTTPCall()` | url, method, executor |
| Agent call | `useAgentCall()` | prompt, tools, executor |
| Fee estimate | `useHTTPFeeEstimate()` | — |
| Error handling | `categorizeError(err)` | Error |

---

## Agent Instructions

**When building a Ritual dApp frontend:**

1. **Start with chain config** — Copy `lib/chain.ts`, `lib/wagmi.ts`, and `app/providers.tsx`
2. **Add wallet connection** — Use `<ConnectWallet />` component
3. **Choose your precompile pattern:**
   - Simple (HTTP/LLM) → Use `useHTTPCall()` pattern
   - Two-phase (Agent) → Use `useAgentCall()` pattern
4. **Track state** — Add `useAsyncJobEvents()` to watch on-chain events
5. **Display status** — Use `<AsyncTransactionStatus />` component
6. **Handle errors** — Use `categorizeError()` for user-friendly messages

**Key files to create:**
- `lib/chain.ts` — Ritual chain definition
- `lib/contracts.ts` — Contract addresses & ABIs
- `stores/asyncTxStore.ts` — State management
- `hooks/useAsyncJobEvents.ts` — Event subscriptions
- `hooks/useHTTPCall.ts` or `hooks/useAgentCall.ts` — Transaction submission
- `components/AsyncTransactionStatus.tsx` — Status display

**Dark-mode design tokens:**
- Background: `bg-black`, `bg-gray-900`
- Borders: `border-gray-800`, `border-gray-700`
- Text: `text-white`, `text-gray-400`, `text-gray-600`
- Accent: `text-green-400`, `border-green-500/50`, `bg-green-500/10`
