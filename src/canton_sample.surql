-- Sample Canton event graph for surrealdb-explorer, in the shape the
-- rust-canton-toolbox event-graph-indexer gives the ledger (see src/canton.rs).
-- Loaded by "Load Canton sample" in the app; view it with the
-- "Canton event graph" visualization.
--
-- The story: a bank issues cash to Bob, a venue offers Bob a ticket, Bob pays
-- for it with the cash, passes the ticket to Carol, and Carol shows it.
--
-- Safe to run repeatedly: records are upserted, and the relations that start
-- at the sample's records are recreated. Other data is kept.

DEFINE TABLE IF NOT EXISTS action TYPE RELATION;
DEFINE TABLE IF NOT EXISTS consequence TYPE RELATION;
DEFINE TABLE IF NOT EXISTS target TYPE RELATION;
DEFINE TABLE IF NOT EXISTS consumes TYPE RELATION;
DEFINE TABLE IF NOT EXISTS requested TYPE RELATION;

-- Parties
UPSERT party:⟨Bank::1220f3a9c1d2⟩  MERGE { party_id: 'Bank::1220f3a9c1d2' };
UPSERT party:⟨Venue::1220b7e4a0f5⟩ MERGE { party_id: 'Venue::1220b7e4a0f5' };
UPSERT party:⟨Bob::12209c5d1e8a⟩   MERGE { party_id: 'Bob::12209c5d1e8a' };
UPSERT party:⟨Carol::1220d2a6b3c7⟩ MERGE { party_id: 'Carol::1220d2a6b3c7' };

-- Transactions
UPSERT transaction:1 MERGE { offset: 1, label: 'TX@1', update_id: '1220a01c5e6f7d8e9f00', command_id: 'issue-cash-1',     workflow_id: '', synchronizer_id: 'global-domain::1220ab', effective_at: '2026-11-02T09:00:00Z', record_time: '2026-11-02T09:00:00Z' };
UPSERT transaction:2 MERGE { offset: 2, label: 'TX@2', update_id: '1220a02d6f708192a3b4', command_id: 'offer-ticket-1',   workflow_id: '', synchronizer_id: 'global-domain::1220ab', effective_at: '2026-11-02T09:05:00Z', record_time: '2026-11-02T09:05:00Z' };
UPSERT transaction:3 MERGE { offset: 3, label: 'TX@3', update_id: '1220a03e7081a2b3c4d5', command_id: 'accept-offer-1',   workflow_id: '', synchronizer_id: 'global-domain::1220ab', effective_at: '2026-11-02T09:12:00Z', record_time: '2026-11-02T09:12:00Z' };
UPSERT transaction:4 MERGE { offset: 4, label: 'TX@4', update_id: '1220a04f8192b3c4d5e6', command_id: 'give-ticket-1',    workflow_id: '', synchronizer_id: 'global-domain::1220ab', effective_at: '2026-11-02T10:30:00Z', record_time: '2026-11-02T10:30:00Z' };
UPSERT transaction:5 MERGE { offset: 5, label: 'TX@5', update_id: '1220a0509203c4d5e6f7', command_id: 'show-ticket-1',    workflow_id: '', synchronizer_id: 'global-domain::1220ab', effective_at: '2026-11-02T19:45:00Z', record_time: '2026-11-02T19:45:00Z' };

-- Created events (contracts)
UPSERT created:⟨00c1a5b2⟩ MERGE { contract_id: '00c1a5b2', label: 'Cash@1',        template_name: 'Main.Cash',        offset: 1, node_id: 0, signatories: ['Bank::1220f3a9c1d2'],  created_at: '2026-11-02T09:00:00Z', create_arg: { issuer: 'Bank::1220f3a9c1d2', owner: 'Bob::12209c5d1e8a', amount: 100.0, currency: 'USD' } };
UPSERT created:⟨00c2b6c3⟩ MERGE { contract_id: '00c2b6c3', label: 'TicketOffer@2', template_name: 'Main.TicketOffer', offset: 2, node_id: 0, signatories: ['Venue::1220b7e4a0f5'], created_at: '2026-11-02T09:05:00Z', create_arg: { organizer: 'Venue::1220b7e4a0f5', buyer: 'Bob::12209c5d1e8a', price: 100.0, event: 'Concert, Nov 2' } };
UPSERT created:⟨00c3c7d4⟩ MERGE { contract_id: '00c3c7d4', label: 'Cash@3',        template_name: 'Main.Cash',        offset: 3, node_id: 2, signatories: ['Bank::1220f3a9c1d2'],  created_at: '2026-11-02T09:12:00Z', create_arg: { issuer: 'Bank::1220f3a9c1d2', owner: 'Venue::1220b7e4a0f5', amount: 100.0, currency: 'USD' } };
UPSERT created:⟨00c4d8e5⟩ MERGE { contract_id: '00c4d8e5', label: 'Ticket@3',      template_name: 'Main.Ticket',      offset: 3, node_id: 3, signatories: ['Venue::1220b7e4a0f5'], created_at: '2026-11-02T09:12:00Z', create_arg: { organizer: 'Venue::1220b7e4a0f5', owner: 'Bob::12209c5d1e8a', event: 'Concert, Nov 2' } };
UPSERT created:⟨00c5e9f6⟩ MERGE { contract_id: '00c5e9f6', label: 'Ticket@4',      template_name: 'Main.Ticket',      offset: 4, node_id: 1, signatories: ['Venue::1220b7e4a0f5'], created_at: '2026-11-02T10:30:00Z', create_arg: { organizer: 'Venue::1220b7e4a0f5', owner: 'Carol::1220d2a6b3c7', event: 'Concert, Nov 2' } };

-- Exercised events (choices); the id is [offset, node_id], the indexer's key
UPSERT exercised:[3, 0] MERGE { offset: 3, node_id: 0, label: 'TicketOffer_Accept@3', choice_name: 'TicketOffer_Accept', target_contract_id: '00c2b6c3', acting_parties: ['Bob::12209c5d1e8a'],   consuming: true,  last_descendant_node_id: 3, result_contract_ids: ['00c4d8e5'], transaction_effective_at: '2026-11-02T09:12:00Z', choice_arg: { cashCid: '00c1a5b2' } };
UPSERT exercised:[3, 1] MERGE { offset: 3, node_id: 1, label: 'Cash_Transfer@3',      choice_name: 'Cash_Transfer',      target_contract_id: '00c1a5b2', acting_parties: ['Bob::12209c5d1e8a'],   consuming: true,  last_descendant_node_id: 2, result_contract_ids: ['00c3c7d4'], transaction_effective_at: '2026-11-02T09:12:00Z', choice_arg: { newOwner: 'Venue::1220b7e4a0f5' } };
UPSERT exercised:[4, 0] MERGE { offset: 4, node_id: 0, label: 'Ticket_Transfer@4',    choice_name: 'Ticket_Transfer',    target_contract_id: '00c4d8e5', acting_parties: ['Bob::12209c5d1e8a'],   consuming: true,  last_descendant_node_id: 1, result_contract_ids: ['00c5e9f6'], transaction_effective_at: '2026-11-02T10:30:00Z', choice_arg: { newOwner: 'Carol::1220d2a6b3c7' } };
UPSERT exercised:[5, 0] MERGE { offset: 5, node_id: 0, label: 'Ticket_Show@5',        choice_name: 'Ticket_Show',        target_contract_id: '00c5e9f6', acting_parties: ['Carol::1220d2a6b3c7'], consuming: false, last_descendant_node_id: 0, result_contract_ids: [],           transaction_effective_at: '2026-11-02T19:45:00Z', choice_arg: { to: 'Venue::1220b7e4a0f5' } };

-- Relations: remove the sample's old ones, then add them again.
LET $sample = [
    party:⟨Bank::1220f3a9c1d2⟩, party:⟨Venue::1220b7e4a0f5⟩, party:⟨Bob::12209c5d1e8a⟩, party:⟨Carol::1220d2a6b3c7⟩,
    transaction:1, transaction:2, transaction:3, transaction:4, transaction:5,
    exercised:[3, 0], exercised:[3, 1], exercised:[4, 0], exercised:[5, 0]
];
DELETE action WHERE in INSIDE $sample;
DELETE consequence WHERE in INSIDE $sample;
DELETE target WHERE in INSIDE $sample;
DELETE consumes WHERE in INSIDE $sample;
DELETE requested WHERE in INSIDE $sample;

-- Who asked for each transaction
RELATE party:⟨Bank::1220f3a9c1d2⟩->requested->transaction:1;
RELATE party:⟨Venue::1220b7e4a0f5⟩->requested->transaction:2;
RELATE party:⟨Bob::12209c5d1e8a⟩->requested->transaction:3;
RELATE party:⟨Bob::12209c5d1e8a⟩->requested->transaction:4;
RELATE party:⟨Carol::1220d2a6b3c7⟩->requested->transaction:5;

-- Root events of each transaction
RELATE transaction:1->action->created:⟨00c1a5b2⟩;
RELATE transaction:2->action->created:⟨00c2b6c3⟩;
RELATE transaction:3->action->exercised:[3, 0];
RELATE transaction:4->action->exercised:[4, 0];
RELATE transaction:5->action->exercised:[5, 0];

-- Events caused by an exercise
RELATE exercised:[3, 0]->consequence->exercised:[3, 1];
RELATE exercised:[3, 1]->consequence->created:⟨00c3c7d4⟩;
RELATE exercised:[3, 0]->consequence->created:⟨00c4d8e5⟩;
RELATE exercised:[4, 0]->consequence->created:⟨00c5e9f6⟩;

-- The contract each exercise acts on, and the ones it archives
RELATE exercised:[3, 0]->target->created:⟨00c2b6c3⟩;
RELATE exercised:[3, 0]->consumes->created:⟨00c2b6c3⟩;
RELATE exercised:[3, 1]->target->created:⟨00c1a5b2⟩;
RELATE exercised:[3, 1]->consumes->created:⟨00c1a5b2⟩;
RELATE exercised:[4, 0]->target->created:⟨00c4d8e5⟩;
RELATE exercised:[4, 0]->consumes->created:⟨00c4d8e5⟩;
RELATE exercised:[5, 0]->target->created:⟨00c5e9f6⟩;
