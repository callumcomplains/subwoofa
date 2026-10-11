package com.dts.encoder.settings;

import java.util.*;

/** Walks the encoder's own configuration cascade (DTSEncConfig.dll) and prints every legal combination. */
public class Enumerate {
    static List<Constants.DestFormat> dests() { return new ArrayList<>(Model.getLegalDestFormats()); }

    public static void main(String[] a) throws Exception {
        Model.initialize(Integer.parseInt(System.getProperty("prod","57")));
        Model.set71LayoutsFiltered(a.length > 0 && a[0].equals("filtered"));
        for (Constants.DestFormat d : Constants.DestFormat.values()) {
            Model.setDestFormat(d);
            for (Constants.StreamType s : Constants.StreamType.values()) {
                Model.setDestFormat(d);
                Model.setStreamType(s);
                if (Model.getDestFormat() != d || Model.getStreamType() != s) { System.err.println("skip " + d + " " + s + " -> " + Model.getDestFormat() + " " + Model.getStreamType()); continue; }
                int nr = Model.getSamplingRateCount();
                List<Integer> rates = new ArrayList<>();
                for (int i = 0; i < nr; i++) rates.add(Model.getSamplingRateValue(i));
                for (int rate : rates) {
                    Model.setDestFormat(d); Model.setStreamType(s); Model.setSamplingRate(rate);
                    for (Constants.ChannelLayout l : new ArrayList<>(Model.getLegalChannelLayouts())) {
                        Model.setDestFormat(d); Model.setStreamType(s); Model.setSamplingRate(rate); Model.setChannelLayout(l);
                        int nb = Model.getBitRateCount();
                        StringBuilder sb = new StringBuilder();
                        for (int j = 0; j < nb; j++) sb.append(sb.length() > 0 ? "," : "").append(Model.getBitRateValue(j));
                        System.out.println(d + "\t" + s + "\t" + rate + "\t" + l + "\t" + sb);
                    }
                }
            }
        }
    }
}
