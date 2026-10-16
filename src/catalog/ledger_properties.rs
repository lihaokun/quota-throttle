use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn groups() -> (TempDir, CatalogStore, [(i64, i64, &'static str); 3]) {
    let (dir, mut store, s, g) = mapped(3);
    let b = store
        .create_group(group("ledger-b", SourceKind::Glm))
        .unwrap()
        .group_id;
    let other = store
        .create_source(source("fake-ledger-other", SourceKind::Glm))
        .unwrap()
        .source_id;
    store
        .conn
        .execute(
            "UPDATE memberships SET group_id=?1 WHERE channel_id=1002",
            [b.get()],
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE memberships SET source_id=?1 WHERE channel_id=1003",
            [other.get()],
        )
        .unwrap();
    (
        dir,
        store,
        [
            (s.get(), g.get(), "ledger-g"),
            (s.get(), b.get(), "ledger-b"),
            (other.get(), g.get(), "ledger-g"),
        ],
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn pb1_cross_batch_dedup_replay_and_reopen(ids in prop::collection::vec(1i64..20,0..45), chunk in 1usize..9, reopen in any::<bool>()) {
        let (dir,mut store)=setup();let mut seen=BTreeSet::new();let mut expected=(0,0,0);let mut canonical=BTreeMap::new();
        for part in ids.chunks(chunk) {
            let t=begin(&mut store);let mut input=Vec::new();let mut fresh=0;
            for &id in part.iter().rev() {
                let mut r=row(id);r["prompt_tokens"]=json!(id+1);r["completion_tokens"]=json!(id%4);r["request_id"]=json!("one-request-multiple-consumption-rows");
                r["created_at"]=json!(id*11);r["token_id"]=json!(100+id);r["model_name"]=json!(format!("m{}",id%3));r["group"]=json!(format!("g{}",id%2));r["is_stream"]=json!(id%2==0);
                r["other"]=json!(json!({"status_code":200,"error_code":format!("code{id}"),"admin_info":{"use_channel":[id.to_string(),id.to_string()]},"stream_status":{"status":"ok","end_reason":"eof"}}).to_string());
                canonical.insert(id,json!({"id":id,"request":"one-request-multiple-consumption-rows","kind":"consume","at":id*11,"channel":null,"token":100+id,"model":format!("m{}",id%3),"group":format!("g{}",id%2),"input":id+1,"output":id%4,"stream":i64::from(id%2==0),"metadata":{"version":1,"state":"parsed","status_code":200,"error_type":null,"error_code":format!("code{id}"),"attempted_channels":[id,id],"stream":"eof"},"source":null,"group_id":null,"issue":"unmapped_channel"}));
                input.extend([r.clone(),r]);
                if seen.insert(id) {fresh+=1;expected.0+=id+1;expected.1+=id%4;expected.2+=1;}
            }
            input.push(json!({"type":7}));
            let c=store.commit_fact_page(&t,&input,PageEnd::ReachedEnd,300).unwrap();
            prop_assert_eq!((c.inserted,c.duplicates),(fresh,part.len()*2-fresh));prop_assert_eq!(total(&store.conn),expected);
            if reopen {drop(store);store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();prop_assert_eq!(total(&store.conn),expected);}
            prop_assert_eq!(full_facts(&store.conn),canonical.values().cloned().collect::<Vec<_>>());
        }
        prop_assert_eq!(store.conn.query_row("SELECT count(*) FROM ledger_facts",[],|r|r.get::<_,i64>(0)).unwrap(),seen.len() as i64);
    }

    #[test]
    fn pb2_late_failure_is_atomic(n in 1usize..10, at in 0usize..10, mode in 0u8..6,field in 0usize..11,tag in "[a-z]{1,8}") {
        let (_dir,mut store)=setup();let t=begin(&mut store);
        let mut seed=row(999);if mode==5 {seed["model_name"]=json!("z");seed["prompt_tokens"]=json!(JSON_SAFE);seed["completion_tokens"]=json!(0);}
        store.commit_fact_page(&t,&[seed],PageEnd::ReachedEnd,300).unwrap();let t=begin(&mut store);let old=image(&store.conn);
        let mut input:Vec<_>=(1..=n).map(|id|{let mut r=row(id as i64);r["model_name"]=json!(format!("m{id}"));r}).collect();
        match mode {
            0=>{let mut conflict=row(999);let name=["created_at","channel","token_id","model_name","group","prompt_tokens","completion_tokens","is_stream","request_id","other","type"][field];conflict[name]=match field {0=>json!(126),1|2=>json!(9),3|4|8=>json!(tag),5|6=>json!(100),7=>json!(true),9=>json!(json!({"error_code":tag}).to_string()),_=>json!(5)};if field==10{conflict["prompt_tokens"]=json!(0);conflict["completion_tokens"]=json!(0);}input.push(conflict);},
            1=>store.conn.execute_batch(&format!("CREATE TRIGGER fault BEFORE INSERT ON ledger_facts WHEN NEW.native_log_id={} BEGIN SELECT RAISE(ABORT,'fixture'); END;",at%n+1)).unwrap(),
            2=>store.conn.execute_batch(&format!("CREATE TRIGGER fault BEFORE INSERT ON ledger_minute_usage WHEN NEW.model='m{}' BEGIN SELECT RAISE(ABORT,'fixture'); END;",at%n+1)).unwrap(),
            3=>store.conn.execute_batch("CREATE TRIGGER fault BEFORE UPDATE ON ledger_checkpoints BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap(),
            4=>store.conn.execute_batch("CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE child(id INTEGER REFERENCES parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fault AFTER INSERT ON ledger_facts BEGIN INSERT INTO child VALUES(1); END;").unwrap(),
            _=>{let mut late=row(10000);late["model_name"]=json!("z");input.push(late);},
        }
        let result=store.commit_fact_page(&t,&input,PageEnd::More,300);
        if mode==5 {prop_assert_eq!(result.err(),Some(LedgerError::Fact(FactError::ArithmeticOverflow)));}else{prop_assert!(result.is_err());}
        prop_assert_eq!(image(&store.conn),old);prop_assert_eq!(store.poisoned,mode==4);
    }

    #[test]
    fn pb3_historical_attribution_preserves_shared_and_unknown(modes in prop::collection::vec((0usize..4,0u8..3),1..30)) {
        let (dir,mut store,mapping)=groups();let t=begin(&mut store);let mut input=Vec::new();let mut expected=Vec::new();
        for (i,&(ch,mode)) in modes.iter().enumerate() {
            let mut r=row(i as i64+1);let known=ch>0;
            let name=if known{mapping[ch-1].2}else{"ledger-g"};
            let raw=match mode {0=>None,1=>Some(name),_=>Some("foreign")};
            r["channel"]=json!(if known{1000+ch as i64}else{9000});r["group"]=json!(raw);
            expected.push((if known{Some(mapping[ch-1].0)}else{None},if known&&mode==1{Some(mapping[ch-1].1)}else{None},raw.map(str::to_owned)));input.push(r);
        }
        store.commit_fact_page(&t,&input,PageEnd::ReachedEnd,300).unwrap();
        let actual:Vec<(Option<i64>,Option<i64>,Option<String>)>=store.conn.prepare("SELECT source_id,group_id,native_group FROM ledger_facts ORDER BY native_log_id").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap().map(|r|r.unwrap()).collect();
        prop_assert_eq!(actual,expected);prop_assert_eq!(total(&store.conn),(5*modes.len() as i64,3*modes.len() as i64,modes.len() as i64));
        let before=image(&store.conn);drop(store);let store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();prop_assert_eq!(image(&store.conn),before);
    }

    #[test]
    fn pb4_joint_cells_match_independent_raw_model(input in prop::collection::vec((0i64..300,0usize..4,0i64..4,0i64..4,0u8..3,0i64..1000,0i64..1000,any::<bool>()),0..40)) {
        type K=(i64,Option<i64>,Option<i64>,Option<i64>,Option<String>,Option<i64>,Option<String>);
        let (_dir,mut store,mapping)=groups();let t=begin(&mut store);let mut rows=Vec::new();let mut expected=BTreeMap::<K,(i64,i64,i64)>::new();let mut sums=(0,0,0);
        for (i,&(at,ch,token,model,mode,raw_in,raw_out,error)) in input.iter().enumerate() {
            let used_in=if error{0}else{raw_in};let used_out=if error{0}else{raw_out};
            let raw_group=match mode{0=>None,1=>Some(if ch>0{mapping[ch-1].2}else{"ledger-g"}.to_owned()),_=>Some("foreign".to_owned())};
            let model=(model>0).then(||format!("m{model}"));let token=(token>0).then_some(token+100);
            let channel=(ch>0).then_some(1000+ch as i64);let source=if ch>0{Some(mapping[ch-1].0)}else{None};let group=if ch>0&&mode==1{Some(mapping[ch-1].1)}else{None};
            let mut r=row(i as i64+1);r["created_at"]=json!(at);r["channel"]=json!(channel.unwrap_or(0));r["group"]=json!(raw_group);r["token_id"]=json!(token.unwrap_or(0));r["model_name"]=json!(model);r["prompt_tokens"]=json!(used_in);r["completion_tokens"]=json!(used_out);r["type"]=json!(if error{5}else{2});rows.push(r);
            if !error {let cell=expected.entry(((at/60)*60,channel,source,group,raw_group,token,model)).or_default();cell.0+=used_in;cell.1+=used_out;cell.2+=1;sums.0+=used_in;sums.1+=used_out;sums.2+=1;}
        }
        store.commit_fact_page(&t,&rows,PageEnd::ReachedEnd,300).unwrap();
        let actual:BTreeMap<K,(i64,i64,i64)>=store.conn.prepare("SELECT minute,channel_id,source_id,group_id,native_group,token_id,model,input,output,consumption_rows FROM ledger_minute_usage").unwrap().query_map([],|r|Ok(((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?),(r.get(7)?,r.get(8)?,r.get(9)?)))).unwrap().map(|r|r.unwrap()).collect();
        prop_assert_eq!(&actual,&expected);prop_assert_eq!(total(&store.conn),sums);
        // 四个边际都包含None桶，独立整数加算，不调用生产投影。
        for dimension in 0..4 {let mut marginal=BTreeMap::<String,(i64,i64,i64)>::new();for (k,v) in &actual {
            let label=match dimension{0=>format!("{:?}",k.6),1=>format!("{:?}",k.2),2=>format!("{:?}",k.3),_=>format!("{:?}",k.5)};
            let x=marginal.entry(label).or_default();x.0+=v.0;x.1+=v.1;x.2+=v.2;
        }prop_assert_eq!(marginal.values().fold((0,0,0),|a,v|(a.0+v.0,a.1+v.1,a.2+v.2)),sums);}
    }

    #[test]
    fn pb5_checkpoint_state_machine_replay_and_restart(actions in prop::collection::vec((0usize..2,0u8..7,any::<bool>()),1..40)) {
        let (dir,mut store)=setup();let mut first=[Some(begin(&mut store)),None];let mut model=[(1,1,1),(0,1,0)];let mut revision=3;let mut used=BTreeSet::new();
        for (index,&(lane,action,repeat)) in actions.iter().enumerate() {
            let lane_id=if lane==0{ScanLane::Recent}else{ScanLane::History};let (generation,page,phase)=&mut model[lane];
            match action {
                0|1|6 if *phase==1=>{let t=store.load_scan_ticket(lane_id).unwrap().unwrap();let id=if repeat{1}else{index as i64+1};let rows=if action==6 {vec![]}else{used.insert(id);vec![row(id)]};store.commit_fact_page(&t,&rows,if action==1{PageEnd::ReachedEnd}else{PageEnd::More},400).unwrap();revision+=1;if action==1{*phase=2}else{*page+=1}},
                2 if *phase==1=>{let t=store.load_scan_ticket(lane_id).unwrap().unwrap();store.mark_scan_fault(&t,ScanFault::Transport,400).unwrap();*phase=3;revision+=1;},
                3=>{let t=store.begin_scan(lane_id,*generation,if lane==0{ScanWindow{from:0,to:300}}else{ScanWindow{from:60,to:360}},360).unwrap();if first[lane].is_none(){first[lane]=Some(t);}
                    if *phase!=1{*generation+=1;*page=1;*phase=1;revision+=1;}},
                4=>{drop(store);store=CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();},
                5 if first[lane].is_some()&&(*generation!=1||*page!=1||*phase!=1)=>{let old=image(&store.conn);prop_assert_eq!(store.commit_fact_page(first[lane].as_ref().unwrap(),&[],PageEnd::More,400).err(),Some(LedgerError::RevisionConflict));prop_assert_eq!(image(&store.conn),old);},
                _=>{},
            }
            let s=store.load_ledger_state().unwrap();prop_assert_eq!(s.revision,revision);
            for (p,&(generation,page,phase)) in s.scans.iter().zip(&model){prop_assert_eq!((p.generation,p.next_page),(generation,page));prop_assert_eq!(p.phase,match phase{0=>ScanPhase::Idle,1=>ScanPhase::Scanning,2=>ScanPhase::Scanned,_=>ScanPhase::Stalled});}
            let count=used.len() as i64;prop_assert_eq!(total(&store.conn),(5*count,3*count,count));
        }
    }
}
