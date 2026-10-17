use super::*;
use proptest::prelude::*;

type Spec = (i64, u8, u8, u8, bool, i64, i64, u8, bool);
fn specs() -> impl Strategy<Value = Vec<Spec>> {
    prop::collection::vec(
        (
            0i64..400_000,
            0u8..4,
            0u8..4,
            0u8..4,
            any::<bool>(),
            0i64..1000,
            0i64..1000,
            0u8..5,
            any::<bool>(),
        ),
        0..60,
    )
}
// 直接由随机原输入算联合分项，不调用解析/归属/计量函数或add_usage生成期望。
fn sample(input: &[Spec]) -> (Vec<Value>, Vec<ChannelAttribution>, UsageDelta) {
    let map = (1..4)
        .map(|c| {
            history(
                10 + c,
                if c < 3 { 701 } else { 702 },
                900 + c,
                &format!("g{c}"),
            )
        })
        .collect();
    let mut rows = Vec::new();
    let mut expected = UsageDelta::default();
    for (i, &(at, ch, model, token, error, raw_in, raw_out, stream, matching)) in
        input.iter().enumerate()
    {
        let (used_in, used_out) = if error { (0, 0) } else { (raw_in, raw_out) };
        let model = (model > 0).then(|| format!("m{model}"));
        let raw_group = if matching {
            Some(format!("g{ch}"))
        } else if at % 2 == 0 {
            None
        } else {
            Some("foreign".into())
        };
        let channel_id = (ch > 0).then(|| ChannelId::new(10 + i64::from(ch)).unwrap());
        let source_id = (ch > 0).then(|| SourceId::new(if ch < 3 { 701 } else { 702 }).unwrap());
        let group_id = (ch > 0 && matching).then(|| GroupId::new(900 + i64::from(ch)).unwrap());
        let token_id = (token > 0).then_some(30 + i64::from(token));
        let mut r = row(i as i64 + 1);
        r["created_at"] = json!(at);
        r["channel"] = json!(if ch == 0 { 0 } else { 10 + i64::from(ch) });
        r["token_id"] = json!(token_id.unwrap_or(0));
        r["model_name"] = json!(model);
        r["group"] = json!(raw_group);
        r["type"] = json!(if error { 5 } else { 2 });
        r["prompt_tokens"] = json!(used_in);
        r["completion_tokens"] = json!(used_out);
        let (status, reason) = match stream {
            0 => ("ok", "done"),
            1 => ("ok", "eof"),
            2 => ("error", "client_gone"),
            3 => ("future", "future"),
            _ => ("ok", "done"),
        };
        if stream < 4 {
            r["is_stream"] = json!(true);
            set_other(
                &mut r,
                json!({"stream_status":{"status":status,"end_reason":reason},"cache_tokens":999999}),
            );
        }
        rows.push(r);
        if error {
            continue;
        }
        let key = UsageCellKey {
            minute: (at / 60) * 60,
            channel_id,
            source_id,
            group_id,
            native_group: raw_group,
            token_id,
            model,
        };
        let cell = expected.cells.entry(key).or_default();
        cell.input += used_in;
        cell.output += used_out;
        cell.consumption_rows += 1;
        expected.total.input += used_in;
        expected.total.output += used_out;
        expected.total.consumption_rows += 1;
    }
    (rows, map, expected)
}
fn marginal<K: Ord>(delta: &UsageDelta, key: impl Fn(&UsageCellKey) -> K) -> UsageTotals {
    let mut groups = BTreeMap::<K, UsageTotals>::new();
    for (k, v) in &delta.cells {
        let g = groups.entry(key(k)).or_default();
        g.input += v.input;
        g.output += v.output;
        g.consumption_rows += v.consumption_rows;
    }
    let mut sum = UsageTotals::default();
    for g in groups.values() {
        sum.input += g.input;
        sum.output += g.output;
        sum.consumption_rows += g.consumption_rows;
    }
    sum
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn p1_parse_preserves_usage_and_redacts_invalid_input(input in 0i64..=JSON_SAFE, raw_output in 0i64..10000,
        mode in 0u8..6, label_is_group in any::<bool>(), core_field in 0usize..8, bad_kind in 0u8..7,
        metadata_mode in 0u8..8, sentinel in "[a-zA-Z0-9]{8,24}") {
        let output=raw_output.min(JSON_SAFE-input);
        let secret=format!("PRIVATE_{sentinel}");let mut r=row(1);
        r["prompt_tokens"]=json!(input);r["completion_tokens"]=json!(output);
        r["content"]=json!(secret);r["authorization"]=json!(secret);
        let malformed=match mode {0=>json!(secret.repeat(257)),1=>json!(true),2=>json!([secret.clone()]),3=>json!("\n"),4=>json!(7),_=>json!({"key":secret})};
        r[if label_is_group{"group"}else{"model_name"}]=malformed;
        set_other(&mut r,json!({"prompt":secret,"key":secret,"messages":[secret.clone()]}));
        let f=decode_native_row(&r).unwrap().unwrap();
        prop_assert_eq!((f.input,f.output),(input,output));
        let degraded=if label_is_group{f.native_group.is_none()}else{f.model.is_none()};
        prop_assert!(degraded);
        prop_assert!(matches!(f.metadata,RelayMetadataState::Unusable));
        let delta=usage(&[r.clone()], &[]);prop_assert_eq!(delta.total,totals(input,output,1));
        let field=["id","type","created_at","channel","token_id","prompt_tokens","completion_tokens","is_stream"][core_field];
        if bad_kind==0 {r.as_object_mut().unwrap().remove(field);}
        else {r[field]=if field=="is_stream" {json!(secret)} else {match bad_kind {
            1=>Value::Null,2=>json!(secret),3=>json!(3.0),4=>json!(true),5=>json!(-1),_=>json!(u64::MAX),
        }};}
        let error=decode_native_row(&r).err().unwrap();
        prop_assert_eq!(error,if bad_kind==0 || field=="is_stream"{FactError::InvalidShape}else{FactError::InvalidNumber});
        let diagnostic=format!("{error:?} {error}");
        prop_assert!(!diagnostic.contains(&secret));
        let mut r=row(2);r["prompt_tokens"]=json!(input);r["completion_tokens"]=json!(output);
        match metadata_mode {
            0=>r["other"]=Value::Null,
            1=>{r.as_object_mut().unwrap().remove("other");}
            2=>set_other(&mut r,json!({"status_code":503,"error_code":"known","unknown_secret":secret})),
            3=>r["other"]=json!(secret),
            4=>set_other(&mut r,json!({"status_code":secret})),
            5=>set_other(&mut r,json!({"admin_info":{"use_channel":[11]}})),
            6=>set_other(&mut r,json!({"stream_status":{"status":"ok","end_reason":"eof"}})),
            _=>r["other"]=json!("x".repeat(MAX_OTHER_BYTES+1)),
        }
        let f=decode_native_row(&r).unwrap().unwrap();
        let actual=match f.metadata {RelayMetadataState::Absent=>0,RelayMetadataState::Parsed(_)=>1,RelayMetadataState::Unusable=>2};
        prop_assert_eq!(actual,match metadata_mode{0|1=>0,2|6=>1,_=>2});
        prop_assert_eq!(usage(&[r],&[]).total,totals(input,output,1));
    }
    #[test]
    fn p2_deduplication_is_order_independent_and_conflicts_are_rejected(values in prop::collection::vec((0i64..1000,0i64..1000),1..40), rotation in 0usize..100) {
        let mut rows=Vec::new();
        for (i,&(input,output)) in values.iter().enumerate(){let mut r=row(i as i64+1);r["prompt_tokens"]=json!(input);r["completion_tokens"]=json!(output);rows.push(r);}
        let mut repeated=rows.iter().cycle().take(rows.len()*3).cloned().collect::<Vec<_>>();let n=repeated.len();repeated.rotate_left(rotation%n);repeated.reverse();
        for r in &mut repeated{r["unknown_payload"]=json!("PRIVATE_IGNORED");}
        let actual=decode_native_batch(&repeated).unwrap();
        prop_assert_eq!(actual.len(),values.len());
        for (i,f) in actual.iter().enumerate(){prop_assert_eq!((f.log_id,f.input,f.output),(i as i64+1,values[i].0,values[i].1));}
        let mut conflict=rows[0].clone();conflict["completion_tokens"]=json!(values[0].1+1);rows.push(conflict);
        prop_assert_eq!(decode_native_batch(&rows).err(),Some(FactError::ConflictingIdentity));
    }
    #[test]
    fn p3_historical_attribution_is_total_and_checks_unused_mappings(modes in prop::collection::vec(0u8..4,1..30)) {
        let mut map=Vec::new();let mut facts=Vec::new();
        for (i,&mode) in modes.iter().enumerate(){
            map.push(history(100+i as i64,700+(i%4) as i64,900+(i%3) as i64,"historical"));
            let mut f=fact(i as i64+1);f.channel_id=Some(ChannelId::new(if mode==0{99_999}else{100+i as i64}).unwrap());
            f.native_group=match mode{1=>Some("historical".into()),2=>Some("changed".into()),_=>None};facts.push(f);
        }
        let actual=attribute_batch(&facts,&map).unwrap();prop_assert_eq!(actual.len(),modes.len());
        for (i,a) in actual.iter().enumerate(){
            prop_assert_eq!(a.source_id.map(|n|n.get()),if modes[i]==0{None}else{Some(700+(i%4) as i64)});
            prop_assert_eq!(a.group_id.map(|n|n.get()),if modes[i]==1{Some(900+(i%3) as i64)}else{None});
        }
        prop_assert_eq!(usage_delta(&actual).unwrap().total,totals(5*modes.len() as i64,3*modes.len() as i64,modes.len() as i64));
        map.push(history(999_999,701,901,"unused"));map.push(history(999_999,702,902,"unused"));
        prop_assert_eq!(attribute_batch(&facts,&map).err(),Some(FactError::InvalidAttribution));
    }
    #[test]
    fn p4_joint_projection_and_all_marginals_conserve_usage(input in specs()) {
        let (rows,map,expected)=sample(&input);let actual=usage(&rows,&map);
        prop_assert!(actual.cells==expected.cells);prop_assert_eq!(actual.total,expected.total);
        prop_assert_eq!(marginal(&actual,|k|k.model.clone()),expected.total);
        prop_assert_eq!(marginal(&actual,|k|k.source_id),expected.total);
        prop_assert_eq!(marginal(&actual,|k|k.group_id),expected.total);
        prop_assert_eq!(marginal(&actual,|k|k.token_id),expected.total);
        prop_assert_eq!(marginal(&actual,|k|k.channel_id),expected.total);
        prop_assert_eq!(marginal(&actual,|k|k.native_group.clone()),expected.total);
    }
    #[test]
    fn p5_disjoint_batches_merge_without_losing_dimensions(input in specs(), chunk in 1usize..16) {
        let (rows,map,expected)=sample(&input);let mut merged=UsageDelta::default();
        for part in rows.chunks(chunk).rev(){
            let delta=usage(part,&map);
            for (key,v) in delta.cells {let cell=merged.cells.entry(key).or_default();cell.input+=v.input;cell.output+=v.output;cell.consumption_rows+=v.consumption_rows;}
            merged.total.input+=delta.total.input;merged.total.output+=delta.total.output;merged.total.consumption_rows+=delta.total.consumption_rows;
        }
        prop_assert!(merged.cells==expected.cells);prop_assert_eq!(merged.total,expected.total);
    }
    #[test]
    fn p6_exact_arithmetic_and_utc_minutes_preserve_boundaries(gap in 0i64..256, input in 0i64..300, output in 0i64..300, at in 0i64..=JSON_SAFE) {
        let a=totals(JSON_SAFE-gap,0,1);let b=totals(input,output,1);
        let total=i128::from(a.input)+i128::from(input)+i128::from(output);
        let result=add_usage(a,b);
        if total>i128::from(JSON_SAFE){prop_assert_eq!(result.err(),Some(FactError::ArithmeticOverflow));}
        else {prop_assert_eq!(result.unwrap(),totals(a.input+input,output,2));}
        let count=add_usage(totals(0,0,JSON_SAFE-gap),totals(0,0,input));
        if input>gap{prop_assert_eq!(count.err(),Some(FactError::ArithmeticOverflow));}else{prop_assert_eq!(count.unwrap().consumption_rows,JSON_SAFE-gap+input);}
        let mut r=row(1);r["created_at"]=json!(at);r["prompt_tokens"]=json!(0);r["completion_tokens"]=json!(0);r["model_name"]=Value::Null;
        let delta=usage(&[r], &[]);prop_assert_eq!(delta.total,totals(0,0,1));
        prop_assert_eq!(delta.cells.keys().next().unwrap().minute,(at/60)*60);
    }
}
