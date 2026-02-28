#!/bin/bash
# PIChain Mining Monitor
while true; do
    clear
    echo "============================================"
    echo "  PIChain Mining Monitor  $(date '+%H:%M:%S')"
    echo "============================================"

    # Node info
    INFO=$(curl -s http://localhost:8314/api/v1/info 2>/dev/null)
    MINING=$(curl -s http://localhost:8314/api/v1/mining/status 2>/dev/null)

    if [ -z "$INFO" ]; then
        echo "  Node: OFFLINE"
        sleep 5
        continue
    fi

    HEIGHT=$(echo "$INFO" | python3 -c "import json,sys; print(json.load(sys.stdin)['block_height'])" 2>/dev/null)
    BURNED=$(echo "$INFO" | python3 -c "import json,sys; b=json.load(sys.stdin)['total_burned']; print(f'{b/1e9:.4f}')" 2>/dev/null)
    MEMPOOL=$(echo "$INFO" | python3 -c "import json,sys; print(json.load(sys.stdin)['mempool_size'])" 2>/dev/null)

    FRONTIER=$(echo "$MINING" | python3 -c "import json,sys; print(f\"{json.load(sys.stdin)['frontier_position']:,}\")" 2>/dev/null)
    DIGITS=$(echo "$MINING" | python3 -c "import json,sys; print(f\"{json.load(sys.stdin)['total_digits_verified']:,}\")" 2>/dev/null)
    RANGES=$(echo "$MINING" | python3 -c "import json,sys; print(json.load(sys.stdin)['total_ranges'])" 2>/dev/null)
    DIFF=$(echo "$MINING" | python3 -c "import json,sys; print(json.load(sys.stdin)['difficulty_bits'])" 2>/dev/null)
    RPD=$(echo "$MINING" | python3 -c "import json,sys; d=json.load(sys.stdin)['reward_per_digit']; print(f'{d/1e9:.9f}')" 2>/dev/null)

    # Miner wallet
    ADDR="a2dd927f21d9d251536c6679c111679790225b69"
    ACCT=$(curl -s "http://localhost:8314/api/v1/account/$ADDR" 2>/dev/null)
    BALANCE=$(echo "$ACCT" | python3 -c "import json,sys; print(json.load(sys.stdin)['balance_pi'])" 2>/dev/null)
    NONCE=$(echo "$ACCT" | python3 -c "import json,sys; print(json.load(sys.stdin)['nonce'])" 2>/dev/null)

    echo ""
    echo "  Node"
    echo "    Block Height:    $HEIGHT"
    echo "    Mempool:         $MEMPOOL pending"
    echo "    Total Burned:    $BURNED PI"
    echo ""
    echo "  Mining (Fixed 8-bit PoW — all hardware welcome)"
    echo "    Frontier:        $FRONTIER hex digits of PI"
    echo "    Total Verified:  $DIGITS hex digits"
    echo "    Proof Ranges:    $RANGES"
    echo "    Min PoW:         $DIFF bits (fixed — any machine can mine)"
    echo "    Reward/digit:    $RPD PI"
    echo ""
    echo "  Miner Wallet"
    echo "    Balance:         $BALANCE PI"
    echo "    Txs Submitted:   $NONCE"
    echo ""

    # Miner process
    MINER_PID=$(pgrep -f pichain-miner | head -1)
    if [ -n "$MINER_PID" ]; then
        CPU=$(ps -p $MINER_PID -o %cpu --no-headers 2>/dev/null | tr -d ' ')
        MEM=$(ps -p $MINER_PID -o rss --no-headers 2>/dev/null | awk '{printf "%.0f", $1/1024}')
        echo "  Miner Process"
        echo "    PID:             $MINER_PID"
        echo "    CPU:             ${CPU}%"
        echo "    Memory:          ${MEM} MB"

        # Last log line
        LAST=$(tail -1 /home/dave/Crypto/pichain/live-data/miner.log 2>/dev/null | sed 's/\x1b\[[0-9;]*m//g' | cut -c26-)
        echo "    Last:            $LAST"
    else
        echo "  Miner: NOT RUNNING"
    fi

    echo ""
    echo "============================================"
    echo "  Profiles: --profile laptop|desktop|server|max"
    echo "  Press Ctrl+C to exit"
    sleep 3
done
