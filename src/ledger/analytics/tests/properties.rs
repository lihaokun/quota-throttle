use super::*;

// 不调用生产分桶/投影/过滤函数计算参考值。
fn local_key(t: i64, offset: i64, width: i64) -> i128 {
    let v = i128::from(t) + i128::from(offset) * 60;
    let w = i128::from(width);
    let remainder = ((v % w) + w) % w;
    (v - remainder) / w
}
fn pick<T>(selector: u8, value: T) -> DimensionFilter<T> {
    match selector % 3 {
        0 => DimensionFilter::Any,
        1 => DimensionFilter::Unknown,
        _ => DimensionFilter::Exact(value),
    }
}
fn accepts<T: PartialEq>(s: u8, actual: Option<T>, wanted: T) -> bool {
    match s % 3 {
        0 => true,
        1 => actual.is_none(),
        _ => actual == Some(wanted),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn pb1_query_partition_and_timezone_match_independent_reference(
        low in 0i64..100000000, span in 1i64..=MAX_QUERY_SECONDS,
        near_max in any::<bool>(), offset in -720i64..=840, daily in any::<bool>(),
        probes in prop::collection::vec(any::<u32>(),0..40)
    ) {
        let from=if near_max {JSON_SAFE-span-low}else{low}; let to=from+span;
        let grain=if daily {UsageGrain::Day}else{UsageGrain::Hour}; let width=if daily {86400}else{3600};
        let q=query(from,to,offset,grain); let parts=q.partition(); let mut all=parts.edges.clone(); all.extend(parts.whole); all.sort_by_key(|w|w.from);
        prop_assert_eq!(all.first().unwrap().from,from); prop_assert_eq!(all.last().unwrap().to,to);
        for pair in all.windows(2) {prop_assert_eq!(pair[0].to,pair[1].from);}
        if let Some(w)=parts.whole {prop_assert_eq!(w.from%60,0);prop_assert_eq!(w.to%60,0);prop_assert!(w.from<w.to);}
        let buckets=q.bucket_ranges(); prop_assert!(buckets.len()<=if daily {32}else{745}, "bucket limit exceeded");
        prop_assert_eq!(buckets[0].from,from); prop_assert_eq!(buckets.last().unwrap().to,to);
        for pair in buckets.windows(2) {prop_assert_eq!(pair[0].to,pair[1].from);}
        for t in [from,to-1,from+span/2].into_iter().chain(probes.into_iter().map(|p|from+i64::from(p)%span)) {
            let minute=t-t%60; let belongs_to_full=minute>=from && minute+60<=to;
            prop_assert_eq!(parts.whole.is_some_and(|w|w.from<=t && t<w.to),belongs_to_full);
            prop_assert_eq!(all.iter().filter(|w|w.from<=t && t<w.to).count(),1);
            let index=(local_key(t,offset,width)-local_key(from,offset,width)) as usize;
            prop_assert_eq!(q.bucket_index(t).unwrap(),index);
            prop_assert!(buckets[index].from<=t && t<buckets[index].to);
        }
        prop_assert_eq!(q.bucket_index(to),Err(AnalyticsError::InvalidQuery));
    }

    #[test]
    fn pb2_analytics_filters_and_all_partitions_match_raw_reference(
        values in prop::collection::vec((0i64..172800,0u8..4,0u8..4,0u8..5,0u8..5,0i64..10000,0i64..10000),0..180),
        selectors in prop::array::uniform4(0u8..3), offset in -720i64..=840, daily in any::<bool>(), base in 0i64..100000
    ) {
        let labels=[None,Some("未知"),Some("M"),Some("m"),Some("<tag>")];
        let f=UsageFilter {source:pick(selectors[0],SourceId::new(2).unwrap()),group:pick(selectors[1],GroupId::new(2).unwrap()),token:pick(selectors[2],2),model:pick(selectors[3],"M".to_owned())};
        let grain=if daily {UsageGrain::Day}else{UsageGrain::Hour}; let width=if daily {86400}else{3600};
        let c=control(); let mut acc=UsageAccumulator::new(UsageQuery::new(ScanWindow {from:base,to:base+172800},offset,grain,f).unwrap(),&c).unwrap();
        let mut total=(0i64,0i64,0i64); let mut models=BTreeMap::new(); let mut tokens=BTreeMap::new(); let mut times=BTreeMap::new();
        for (t,s,g,k,m,input,output) in values {
            let source=(s!=0).then_some(i64::from(s)); let group=if s!=0 && g!=0 {Some(i64::from(g))}else{None};
            let token=(k!=0).then_some(i64::from(k)); let model=labels[m as usize]; let at=base+t;
            acc.push(UsageDimensions {source:source.map(|n|SourceId::new(n).unwrap()),group:group.map(|n|GroupId::new(n).unwrap()),token,model},at,amount(input,output,1),&c).unwrap();
            if !(accepts(selectors[0],source,2)&&accepts(selectors[1],group,2)&&accepts(selectors[2],token,2)&&accepts(selectors[3],model,"M")) {continue;}
            total.0+=input;total.1+=output;total.2+=1;
            for item in [models.entry(model.map(str::to_owned)).or_insert((0,0,0)),tokens.entry(token).or_insert((0,0,0)),times.entry((local_key(at,offset,width),model.map(str::to_owned))).or_insert((0,0,0))] {item.0+=input;item.1+=output;item.2+=1;}
        }
        let r=report(acc,&c); prop_assert_eq!(r.total,amount(total.0,total.1,total.2));
        let convert=|(a,b,c)|amount(a,b,c);
        prop_assert_eq!(r.models,models.into_iter().map(|(k,v)|(k,convert(v))).collect::<Vec<_>>());
        prop_assert_eq!(r.tokens,tokens.into_iter().map(|(k,v)|(k,convert(v))).collect::<Vec<_>>());
        for b in r.buckets {
            let key=local_key(b.from,offset,width);
            let expected:Vec<_>=times.iter().filter(|((t,_),_)|*t==key).map(|((_,m),v)|(m.clone(),convert(*v))).collect();
            let sum=expected.iter().fold((0,0,0),|a,(_,b)|(a.0+b.input,a.1+b.output,a.2+b.consumption_rows));
            prop_assert_eq!(b.models,expected);prop_assert_eq!(b.total,convert(sum));
        }
    }

    #[test]
    fn analytics_pbt_minute_deltas_equal_individual_consumption(
        input in 0i64..100000,output in 0i64..100000,rows in 1i64..500,
        offset in -720i64..=840,minute in 0i64..40000,model in prop::option::of("[a-z]{1,12}"),token in prop::option::of(1i64..100)
    ) {
        let at=minute*60; let q=query(at,at+60,offset,UsageGrain::Hour); let c=control();
        let mut single=UsageAccumulator::new(q.clone(),&c).unwrap(); let mut combined=UsageAccumulator::new(q,&c).unwrap();
        for i in 0..rows { single.push(dimensions(model.as_deref(),token),at+i%60,amount(input,output,1),&c).unwrap(); }
        combined.push(dimensions(model.as_deref(),token),at,amount(input*rows,output*rows,rows),&c).unwrap();
        prop_assert!(report(single,&c)==report(combined,&c));
    }

    #[test]
    fn analytics_pbt_cancel_or_overflow_never_publishes_partial_result(
        count in 1usize..200,fail_at in 0usize..220,cancel in any::<bool>(),tail in 0usize..5
    ) {
        let c=control(); let mut acc=UsageAccumulator::new(query(0,60,0,UsageGrain::Hour),&c).unwrap();
        let expected=if fail_at<count {Some(if cancel {AnalyticsError::Cancelled}else{AnalyticsError::ArithmeticOverflow})}else{None};
        for i in 0..count {
            let totals=if i==fail_at && !cancel {amount(JSON_SAFE,1,1)}else{amount(1,2,1)};
            if i==fail_at && cancel {c.cancel();}
            let actual=acc.push(dimensions(None,None),0,totals,&c);
            if i==fail_at {prop_assert_eq!(actual,Err(expected.clone().unwrap()));break;}
            prop_assert_eq!(actual,Ok(()));
        }
        if let Some(e)=expected {
            for _ in 0..tail {prop_assert_eq!(acc.push(dimensions(None,None),0,amount(1,0,1),&c),Err(e.clone()));}
            prop_assert_eq!(acc.finish(state(),&c).err(),Some(e));
        } else {prop_assert_eq!(report(acc,&c).total,amount(count as i64,2*count as i64,count as i64));}
    }
}
